//! A full-screen PDF viewer. Poppler renders each page, and the kitty graphics protocol, which
//! Ghostty supports, draws it as a real image. Other terminals get the PDF's text instead.

use anyhow::{bail, Context, Result};
use base64::Engine;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::{cursor, execute, terminal};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

const CHUNK: usize = 4096;
const CACHED_PAGES: usize = 10;
const MAX_SIDE: f64 = 8000.0;
const MIN_ZOOM: f64 = 0.25;
const MAX_ZOOM: f64 = 6.0;
const BG: &str = "\x1b[48;2;31;30;29m";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fit {
    Page,
    Width,
}

pub fn is_pdf(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// Terminals that are known to speak the kitty graphics protocol.
pub fn graphics_supported() -> bool {
    let has = |name: &str| std::env::var(name).unwrap_or_default().to_lowercase();
    let known = |v: String| v.contains("ghostty") || v.contains("kitty") || v.contains("wezterm");
    known(has("TERM")) || known(has("TERM_PROGRAM")) || std::env::var_os("KITTY_WINDOW_ID").is_some()
}

/// Reads the `Page N size: W x H pts` lines from `pdfinfo -f 1 -l <last>`.
pub fn parse_page_sizes(info: &str) -> Vec<(f64, f64)> {
    info.lines()
        .filter(|l| l.starts_with("Page ") && l.contains(" size:"))
        .filter_map(|l| {
            let mut words = l.split_once("size:")?.1.split_whitespace();
            let w = words.next()?.parse().ok()?;
            words.next()?;
            let h = words.next()?.parse().ok()?;
            Some((w, h))
        })
        .collect()
}

fn poppler(program: &str) -> Command {
    Command::new(crate::config::find_bin(program))
}

fn page_sizes(path: &Path) -> Result<Vec<(f64, f64)>> {
    let out = poppler("pdfinfo").args(["-f", "1", "-l", "100000"]).arg(path).stdin(Stdio::null()).output().context("pdfinfo isn't available. Install poppler")?;
    if !out.status.success() {
        let reason = String::from_utf8_lossy(&out.stderr);
        bail!("Couldn't read this PDF: {}", reason.trim().lines().last().unwrap_or("unknown error"));
    }
    let sizes = parse_page_sizes(&String::from_utf8_lossy(&out.stdout));
    if sizes.is_empty() {
        bail!("This PDF has no pages");
    }
    Ok(sizes)
}

fn render_page(path: &Path, page: usize, w: u32, h: u32) -> Result<Vec<u8>> {
    let out = poppler("pdftoppm")
        .args(["-png", "-singlefile", "-f", &(page + 1).to_string(), "-l", &(page + 1).to_string()])
        .args(["-scale-to-x", &w.to_string(), "-scale-to-y", &h.to_string()])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("pdftoppm isn't available. Install poppler")?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("Couldn't render page {}", page + 1);
    }
    Ok(out.stdout)
}

pub fn all_text(path: &Path) -> Vec<String> {
    let out = poppler("pdftotext").args(["-layout"]).arg(path).arg("-").stdin(Stdio::null()).stderr(Stdio::null()).output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).split('\u{c}').map(str::to_string).collect(),
        _ => vec![],
    }
}

pub fn page_count(path: &Path) -> Result<usize> {
    Ok(page_sizes(path)?.len())
}

/// Page count and first-page text, for the preview column in the Notes tab.
pub fn summary(path: &Path) -> Option<(usize, String)> {
    let pages = page_sizes(path).ok()?.len();
    let out = poppler("pdftotext").args(["-f", "1", "-l", "1", "-layout"]).arg(path).arg("-").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    Some((pages, String::from_utf8_lossy(&out.stdout).trim_end().to_string()))
}

/// The pixel size to render a page at, keeping its shape.
pub fn target_size(page: (f64, f64), view: (u32, u32), fit: Fit, zoom: f64) -> (u32, u32) {
    let (pw, ph) = (page.0.max(1.0), page.1.max(1.0));
    let (vw, vh) = (view.0.max(1) as f64, view.1.max(1) as f64);
    let base = match fit {
        Fit::Page => (vw / pw).min(vh / ph),
        Fit::Width => vw / pw,
    };
    let limit = (MAX_SIDE / pw).min(MAX_SIDE / ph);
    let scale = (base * zoom).min(limit);
    (((pw * scale).round() as u32).max(1), ((ph * scale).round() as u32).max(1))
}

/// The image split into the base64 chunks the kitty protocol expects.
pub fn transmit_sequence(image_id: u32, png: &[u8]) -> Vec<u8> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(png);
    let mut out = Vec::with_capacity(encoded.len() + encoded.len() / 64);
    let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(CHUNK).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        let head = if i == 0 { format!("\x1b_Ga=t,f=100,i={image_id},q=2,m={more};") } else { format!("\x1b_Gm={more};") };
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

/// Pages containing `query`, ignoring case.
pub fn search_pages(pages: &[String], query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    pages.iter().enumerate().filter(|(_, p)| p.to_lowercase().contains(&q)).map(|(i, _)| i).collect()
}

struct Screen;

impl Screen {
    fn enter() -> Result<Screen> {
        terminal::enable_raw_mode()?;
        execute!(std::io::stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;
        Ok(Screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b_Ga=d,d=A,q=2\x1b\\\x1b[0m");
        let _ = execute!(out, cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

struct Viewer {
    path: PathBuf,
    name: String,
    sizes: Vec<(f64, f64)>,
    page: usize,
    fit: Fit,
    zoom: f64,
    sx: u32,
    sy: u32,
    text_mode: bool,
    can_draw: bool,
    text: Option<Vec<String>>,
    text_scroll: usize,
    cache: HashMap<(usize, u32, u32), Vec<u8>>,
    order: VecDeque<(usize, u32, u32)>,
    in_flight: HashSet<(usize, u32, u32)>,
    tx: Sender<((usize, u32, u32), Option<Vec<u8>>)>,
    rx: Receiver<((usize, u32, u32), Option<Vec<u8>>)>,
    sent: Option<(usize, u32, u32)>,
    count: Option<usize>,
    pending_g: bool,
    msg: String,
    matches: Vec<usize>,
    query: String,
    typing: Option<String>,
    view_px: (u32, u32),
    size_px: (u32, u32),
}

impl Viewer {
    fn dims(&self) -> (u16, u16, u32, u32) {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        let size = terminal::window_size().ok();
        let px_w = size.as_ref().map_or(0, |s| s.width as u32);
        let px_h = size.as_ref().map_or(0, |s| s.height as u32);
        // Some terminals don't report pixels, so fall back to a typical cell size.
        let cell_w = if px_w > 0 { px_w / cols.max(1) as u32 } else { 10 };
        let cell_h = if px_h > 0 { px_h / rows.max(1) as u32 } else { 20 };
        (cols, rows, cell_w.max(1), cell_h.max(1))
    }

    fn page_size(&self) -> (f64, f64) {
        self.sizes[self.page.min(self.sizes.len() - 1)]
    }

    fn store(&mut self, key: (usize, u32, u32), png: Vec<u8>) {
        self.cache.insert(key, png);
        self.order.push_back(key);
        while self.order.len() > CACHED_PAGES {
            if let Some(old) = self.order.pop_front() {
                if Some(old) != self.sent {
                    self.cache.remove(&old);
                }
            }
        }
    }

    fn prefetch(&mut self, page: usize) {
        if page >= self.sizes.len() {
            return;
        }
        let (w, h) = target_size(self.sizes[page], self.view_px, self.fit, self.zoom);
        let key = (page, w, h);
        if self.cache.contains_key(&key) || !self.in_flight.insert(key) {
            return;
        }
        let (tx, path) = (self.tx.clone(), self.path.clone());
        std::thread::spawn(move || {
            let _ = tx.send((key, render_page(&path, page, w, h).ok()));
        });
    }

    fn drain_prefetch(&mut self) {
        while let Ok((key, png)) = self.rx.try_recv() {
            self.in_flight.remove(&key);
            if let Some(png) = png {
                self.store(key, png);
            }
        }
    }

    fn overflow(&self) -> (u32, u32) {
        (self.size_px.0.saturating_sub(self.view_px.0), self.size_px.1.saturating_sub(self.view_px.1))
    }

    fn draw(&mut self) -> Result<()> {
        let mut out = std::io::BufWriter::new(std::io::stdout());
        let (cols, rows, cell_w, cell_h) = self.dims();
        self.view_px = (cols as u32 * cell_w, rows.saturating_sub(1).max(1) as u32 * cell_h);
        write!(out, "{BG}\x1b[2J\x1b_Ga=d,d=a,q=2\x1b\\")?;

        if self.text_mode {
            self.draw_text(&mut out, cols, rows)?;
        } else {
            let (w, h) = target_size(self.page_size(), self.view_px, self.fit, self.zoom);
            self.size_px = (w, h);
            let key = (self.page, w, h);
            if !self.cache.contains_key(&key) {
                self.status(&mut out, cols, rows, "rendering…")?;
                out.flush()?;
                let png = render_page(&self.path, self.page, w, h)?;
                self.store(key, png);
            }
            if self.sent != Some(key) {
                out.write_all(&transmit_sequence(1, &self.cache[&key]))?;
                self.sent = Some(key);
            }
            let (over_x, over_y) = self.overflow();
            self.sx = self.sx.min(over_x);
            self.sy = self.sy.min(over_y);
            let (vis_w, vis_h) = (w.min(self.view_px.0), h.min(self.view_px.1));
            let col = (self.view_px.0 - vis_w) / 2 / cell_w;
            let row = (self.view_px.1 - vis_h) / 2 / cell_h;
            write!(out, "\x1b[{};{}H\x1b_Ga=p,i=1,p=1,x={},y={},w={vis_w},h={vis_h},C=1,q=2\x1b\\", row + 1, col + 1, self.sx, self.sy)?;
            self.prefetch(self.page + 1);
            if self.page > 0 {
                self.prefetch(self.page - 1);
            }
        }
        let note = self.msg.clone();
        self.status(&mut out, cols, rows, &note)?;
        out.flush()?;
        Ok(())
    }

    fn draw_text(&mut self, out: &mut impl Write, cols: u16, rows: u16) -> Result<()> {
        if self.text.is_none() {
            self.text = Some(all_text(&self.path));
        }
        let body = self.text.as_ref().and_then(|t| t.get(self.page)).map_or("", String::as_str);
        let lines: Vec<&str> = body.lines().collect();
        let height = rows.saturating_sub(1) as usize;
        self.text_scroll = self.text_scroll.min(lines.len().saturating_sub(height));
        write!(out, "\x1b[38;2;250;249;245m")?;
        if lines.iter().all(|l| l.trim().is_empty()) {
            write!(out, "\x1b[2;3HThis page has no text. It may be a scan or an image.")?;
        }
        for (i, line) in lines.iter().skip(self.text_scroll).take(height).enumerate() {
            let clipped: String = line.chars().take(cols as usize).collect();
            write!(out, "\x1b[{};1H{clipped}", i + 1)?;
        }
        Ok(())
    }

    fn status(&self, out: &mut impl Write, cols: u16, rows: u16, note: &str) -> Result<()> {
        let bar = "\x1b[48;2;42;41;38m\x1b[38;2;176;174;165m";
        let badge = "\x1b[48;2;217;119;87m\x1b[38;2;31;30;29m\x1b[1m";
        let mode = if self.text_mode {
            "text".to_string()
        } else {
            let label = if self.fit == Fit::Width { "width" } else { "page" };
            format!("fit {label} {:.0}%", self.zoom * 100.0)
        };
        let page = format!(" {}/{} ", self.page + 1, self.sizes.len());
        let right = match &self.typing {
            Some(q) => format!("/{q}"),
            None if !note.is_empty() => note.to_string(),
            None => "j k scroll  J K page  + - zoom  w width  t text  / find  o open  q back".to_string(),
        };
        let left = format!(" {mode}  {}", self.name);
        let used = page.chars().count() + right.chars().count() + 1;
        let room = (cols as usize).saturating_sub(used);
        let left: String = format!("{left:<room$}").chars().take(room).collect();
        write!(out, "\x1b[{rows};1H{badge}{page}\x1b[0m{bar}{left}{right} \x1b[0m{BG}")?;
        Ok(())
    }

    fn go_to(&mut self, page: usize) {
        self.page = page.min(self.sizes.len() - 1);
        self.sy = 0;
        self.sx = 0;
        self.text_scroll = 0;
    }

    /// Moves down the page, then on to the next one, like reading.
    fn scroll(&mut self, dir: i64, amount: u32) {
        if self.text_mode {
            let lines = (amount / 20).max(1) as usize;
            if dir > 0 {
                self.text_scroll += lines;
            } else if self.text_scroll == 0 && self.page > 0 {
                self.go_to(self.page - 1);
            } else {
                self.text_scroll = self.text_scroll.saturating_sub(lines);
            }
            return;
        }
        let (_, over) = self.overflow();
        if dir > 0 {
            if self.sy < over {
                self.sy = (self.sy + amount).min(over);
            } else if self.page + 1 < self.sizes.len() {
                self.go_to(self.page + 1);
            }
        } else if self.sy > 0 {
            self.sy = self.sy.saturating_sub(amount);
        } else if self.page > 0 {
            self.go_to(self.page - 1);
            // Arriving from below starts at the bottom of the previous page.
            let (w, h) = target_size(self.page_size(), self.view_px, self.fit, self.zoom);
            self.size_px = (w, h);
            self.sy = self.overflow().1;
        }
    }

    fn set_zoom(&mut self, zoom: f64) {
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.sx = 0;
        self.sy = 0;
    }

    fn find(&mut self, query: String) {
        if self.text.is_none() {
            self.text = Some(all_text(&self.path));
        }
        self.matches = search_pages(self.text.as_deref().unwrap_or(&[]), &query);
        self.msg = if self.matches.is_empty() {
            format!("No match for {query}. Scanned pages can't be searched")
        } else {
            format!("{query}: on {} page(s). n and N jump", self.matches.len())
        };
        self.query = query;
        if let Some(first) = self.matches.iter().find(|p| **p >= self.page).or(self.matches.first()).copied() {
            self.go_to(first);
        }
    }

    fn next_match(&mut self, forward: bool) {
        let hit = if forward {
            self.matches.iter().find(|p| **p > self.page).or(self.matches.first())
        } else {
            self.matches.iter().rev().find(|p| **p < self.page).or(self.matches.last())
        };
        match hit.copied() {
            Some(p) => self.go_to(p),
            None => self.msg = "Press / to find text first".into(),
        }
    }

    /// Returns true when it's time to leave the viewer.
    fn on_key(&mut self, key: KeyEvent) -> bool {
        if let Some(q) = &mut self.typing {
            match key.code {
                KeyCode::Esc => self.typing = None,
                KeyCode::Enter => {
                    let query = q.trim().to_string();
                    self.typing = None;
                    if !query.is_empty() {
                        self.find(query);
                    }
                }
                KeyCode::Backspace => {
                    q.pop();
                }
                KeyCode::Char(c) => q.push(c),
                _ => {}
            }
            return false;
        }
        self.msg.clear();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let was_g = std::mem::take(&mut self.pending_g);
        let n = self.count.take();
        let times = n.unwrap_or(1).max(1);
        let step = (self.view_px.1 / 8).max(20);
        let half = (self.view_px.1 / 2).max(20);
        match key.code {
            KeyCode::Char('c') if ctrl => return true,
            KeyCode::Char('d') if ctrl => self.scroll(1, half),
            KeyCode::Char('u') if ctrl => self.scroll(-1, half),
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char(c) if c.is_ascii_digit() && (c != '0' || n.is_some()) => {
                self.count = Some((n.unwrap_or(0) * 10 + c.to_digit(10).unwrap_or(0) as usize).min(99999));
            }
            KeyCode::Char('g') if was_g => self.go_to(n.unwrap_or(1).saturating_sub(1)),
            KeyCode::Char('g') => {
                self.pending_g = true;
                self.count = n;
            }
            KeyCode::Char('G') => self.go_to(n.map_or(usize::MAX, |v| v.saturating_sub(1))),
            KeyCode::Char('j') | KeyCode::Down => (0..times).for_each(|_| self.scroll(1, step)),
            KeyCode::Char('k') | KeyCode::Up => (0..times).for_each(|_| self.scroll(-1, step)),
            KeyCode::Char('J') | KeyCode::Char(' ') | KeyCode::PageDown => self.go_to(self.page + times),
            KeyCode::Char('K') | KeyCode::PageUp => self.go_to(self.page.saturating_sub(times)),
            KeyCode::Char('h') | KeyCode::Left => self.sx = self.sx.saturating_sub(step),
            KeyCode::Char('l') | KeyCode::Right => self.sx = (self.sx + step).min(self.overflow().0),
            KeyCode::Char('+') | KeyCode::Char('=') => self.set_zoom(self.zoom * 1.2),
            KeyCode::Char('-') => self.set_zoom(self.zoom / 1.2),
            KeyCode::Char('0') => self.set_zoom(1.0),
            KeyCode::Char('w') => {
                self.fit = if self.fit == Fit::Page { Fit::Width } else { Fit::Page };
                self.set_zoom(1.0);
            }
            KeyCode::Char('t') => {
                if self.can_draw {
                    self.text_mode = !self.text_mode;
                    self.sent = None;
                } else {
                    self.msg = "This terminal can't show images, so only text is available".into();
                }
            }
            KeyCode::Char('/') => self.typing = Some(String::new()),
            KeyCode::Char('n') => self.next_match(true),
            KeyCode::Char('N') => self.next_match(false),
            KeyCode::Char('o') => {
                let _ = Command::new("xdg-open").arg(&self.path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                self.msg = "Opened with your default app".into();
            }
            _ => {}
        }
        false
    }
}

pub fn view(path: &Path) -> Result<()> {
    let path = path.canonicalize().with_context(|| format!("Can't open {}", path.display()))?;
    let sizes = page_sizes(&path)?;
    let can_draw = graphics_supported();
    let (tx, rx) = channel();
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut v = Viewer {
        path,
        name,
        sizes,
        page: 0,
        fit: Fit::Page,
        zoom: 1.0,
        sx: 0,
        sy: 0,
        text_mode: !can_draw,
        can_draw,
        text: None,
        text_scroll: 0,
        cache: HashMap::new(),
        order: VecDeque::new(),
        in_flight: HashSet::new(),
        tx,
        rx,
        sent: None,
        count: None,
        pending_g: false,
        msg: String::new(),
        matches: vec![],
        query: String::new(),
        typing: None,
        view_px: (800, 600),
        size_px: (0, 0),
    };
    let _screen = Screen::enter()?;
    v.draw()?;
    loop {
        v.drain_prefetch();
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if v.on_key(key) {
                    return Ok(());
                }
            }
            Event::Resize(..) => v.sent = None,
            _ => continue,
        }
        v.drain_prefetch();
        v.draw()?;
    }
}

/// What `MOODLE_OPENER` and `lockin open` run: PDFs stay here and the rest go to your usual apps.
pub fn open_any(path: &Path) -> Result<()> {
    if is_pdf(path) {
        return view(path);
    }
    Command::new("xdg-open").arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().context("xdg-open isn't available")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_sizes_are_read_from_pdfinfo() {
        let info = "Title: x\nPages:           2\nPage    1 size:  612 x 792 pts (letter)\nPage    2 size:  841.89 x 595.28 pts (A4)\n";
        assert_eq!(parse_page_sizes(info), vec![(612.0, 792.0), (841.89, 595.28)]);
        assert!(parse_page_sizes("Pages: 0\n").is_empty());
    }

    #[test]
    fn pages_fit_the_view_and_keep_their_shape() {
        // A portrait page in a wide window is limited by height.
        assert_eq!(target_size((600.0, 800.0), (1600, 800), Fit::Page, 1.0), (600, 800));
        // Fit to width fills the width, so the page is taller than the view and scrolls.
        assert_eq!(target_size((600.0, 800.0), (1600, 800), Fit::Width, 1.0), (1600, 2133));
        assert_eq!(target_size((600.0, 800.0), (1600, 800), Fit::Page, 2.0), (1200, 1600));
        // A huge zoom can't ask for an absurd image, and a broken size can't divide by zero.
        let (w, h) = target_size((600.0, 800.0), (4000, 4000), Fit::Page, 6.0);
        assert!(w <= 8000 && h <= 8000);
        let (w, h) = target_size((0.0, 0.0), (0, 0), Fit::Page, 1.0);
        assert!(w >= 1 && h >= 1);
    }

    #[test]
    fn images_are_sent_in_protocol_sized_chunks() {
        let seq = transmit_sequence(1, &vec![7u8; 10_000]);
        let text = String::from_utf8(seq).unwrap();
        let parts: Vec<&str> = text.split("\x1b\\").filter(|p| !p.is_empty()).collect();
        assert!(parts.len() >= 3);
        assert!(parts[0].starts_with("\x1b_Ga=t,f=100,i=1,q=2,m=1;"));
        assert!(parts[1].starts_with("\x1b_Gm=1;"));
        assert!(parts.last().unwrap().starts_with("\x1b_Gm=0;"));
        let mut joined = String::new();
        for p in &parts {
            let body = p.split(';').nth(1).unwrap();
            assert!(body.len() <= CHUNK);
            joined += body;
        }
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(joined).unwrap(), vec![7u8; 10_000]);
        // A tiny image fits one chunk, which must say that nothing follows.
        assert!(String::from_utf8(transmit_sequence(1, b"x")).unwrap().starts_with("\x1b_Ga=t,f=100,i=1,q=2,m=0;"));
    }

    #[test]
    fn search_ignores_case_and_reports_pages() {
        let pages = vec!["Intro".to_string(), "Die FUNKTION".to_string(), "".to_string(), "funktion again".to_string()];
        assert_eq!(search_pages(&pages, "funktion"), vec![1, 3]);
        assert!(search_pages(&pages, "absent").is_empty());
    }

    #[test]
    fn pdf_extension_ignores_case() {
        assert!(is_pdf(Path::new("/a/B.PDF")) && is_pdf(Path::new("x.pdf")));
        assert!(!is_pdf(Path::new("x.pdf.md")) && !is_pdf(Path::new("pdf")));
    }
}
