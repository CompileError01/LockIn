use crate::app::*;
use crate::chats::{BookRow, Level as ChatLevel};
use crate::db::{Grade, Task};
use crate::files::{human_size, Entry, Preview, Sort};
use crate::form::Target;
use crate::settings::{self, Kind};
use crate::tutor::{self, Material, Role};
use crate::timeutil::{date_of, fmt_abs, fmt_rel, local, now_ts, today};
use chrono::{Datelike, Duration, Local};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// Claude's palette: warm near-black, ivory text and terracotta as the one loud colour.
const BG: Color = Color::Rgb(0x1f, 0x1e, 0x1d);
const PANEL: Color = Color::Rgb(0x2a, 0x29, 0x26);
const SEL: Color = Color::Rgb(0x3a, 0x38, 0x34);
const BORDER: Color = Color::Rgb(0x45, 0x43, 0x3e);
const FG: Color = Color::Rgb(0xfa, 0xf9, 0xf5);
const MUTED: Color = Color::Rgb(0xb0, 0xae, 0xa5);
const DIM: Color = Color::Rgb(0x78, 0x76, 0x6e);
const ORANGE: Color = Color::Rgb(0xd9, 0x77, 0x57);
const BLUE: Color = Color::Rgb(0x6a, 0x9b, 0xcc);
const GREEN: Color = Color::Rgb(0x8f, 0xa8, 0x76);
const YELLOW: Color = Color::Rgb(0xd9, 0xa8, 0x57);
const RED: Color = Color::Rgb(0xd4, 0x64, 0x5a);

fn st(fg: Color) -> Style {
    Style::default().fg(fg)
}

/// Cuts text to a display width, so umlauts and wide characters can't break a column.
fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

fn pad(text: &str, width: usize) -> String {
    let cut = fit(text, width);
    let fill = width.saturating_sub(cut.width());
    format!("{cut}{}", " ".repeat(fill))
}

/// `POSE_OO_2IHIF_2627` becomes `POSE` and `AM2IHIF2026/27` becomes `AM`.
pub fn course_tag(course: &str) -> String {
    let tag: String = course.chars().take_while(|c| c.is_ascii_uppercase() || c.is_ascii_lowercase()).collect();
    let tag = if tag.len() >= 2 { tag } else { course.to_string() };
    tag.chars().take(8).collect()
}

fn panel<'a>(title: impl Into<Span<'a>>) -> Block<'a> {
    Block::default().borders(Borders::ALL).border_style(st(BORDER)).title(title.into()).title_style(st(ORANGE).add_modifier(Modifier::BOLD))
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), area);
    let [tabs, body, status, cmd] = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)]).areas(area);
    draw_tabs(f, app, tabs);
    match app.view {
        View::Today => draw_today(f, app, body),
        View::Tasks => draw_tasks(f, app, body),
        View::Calendar => draw_calendar(f, app, body),
        View::Notes => draw_notes(f, app, body),
        View::Habits => draw_habits(f, app, body),
        View::Grades => draw_grades(f, app, body),
        View::Focus => draw_focus(f, app, body),
        View::Claude => draw_claude(f, app, body),
        View::Tutor => draw_tutor(f, app, body),
        View::Chats => draw_chats(f, app, body),
        View::Settings => draw_settings(f, app, body),
    }
    draw_status(f, app, status);
    draw_cmdline(f, app, cmd);
    if app.pending == Some(' ') {
        draw_which_key(f, body, "Space", LEADER_KEYS);
    } else if app.pending == Some('g') {
        draw_which_key(f, body, "g", G_KEYS);
    }
    if app.mode == Mode::Form {
        draw_form(f, app, body);
    }
    if app.help_open {
        draw_help(f, app, body);
    }
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::styled(" ✻ LockIn ", Style::default().fg(BG).bg(ORANGE).add_modifier(Modifier::BOLD)), Span::raw(" ")];
    for (i, (view, name)) in VIEWS.iter().enumerate() {
        let label = format!(" {} {name} ", i + 1);
        let style = if *view == app.view {
            Style::default().fg(FG).bg(SEL).add_modifier(Modifier::BOLD)
        } else {
            st(DIM)
        };
        spans.push(Span::styled(label, style));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(PANEL)), area);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let (label, color) = match &app.mode {
        Mode::Normal => ("NORMAL", ORANGE),
        Mode::Input(Prompt::Command) => ("COMMAND", BLUE),
        Mode::Input(Prompt::Search) => ("SEARCH", YELLOW),
        Mode::Input(_) => ("INSERT", GREEN),
        Mode::Confirm(_) => ("CONFIRM", RED),
        Mode::Form => ("INSERT", GREEN),
    };
    let name = VIEWS.iter().find(|(v, _)| *v == app.view).map_or("", |(_, n)| n);
    let mut left = vec![
        Span::styled(format!(" {label} "), Style::default().fg(BG).bg(color).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {} ", name.to_lowercase()), st(FG)),
    ];
    if !app.search.is_empty() {
        left.push(Span::styled(format!("/{} ", app.search), st(YELLOW)));
    }
    if app.view == View::Tasks && app.filter != TaskFilter::All {
        left.push(Span::styled(if app.filter == TaskFilter::Moodle { "[moodle] " } else { "[mine] " }, st(BLUE)));
    }

    const SPIN: [&str; 4] = ["◐", "◓", "◑", "◒"];
    let spin = SPIN[app.spinner / 2 % 4];
    let mut right = Vec::new();
    if let Some(p) = &app.focus {
        let r = p.remaining();
        let icon = if p.paused { "⏸" } else { "●" };
        right.push(Span::styled(format!("{icon} {:02}:{:02}  ", r / 60, r % 60), st(if p.phase == Phase::Work { ORANGE } else { GREEN })));
    }
    if app.syncing {
        right.push(Span::styled(format!("{spin} sync  "), st(BLUE)));
    }
    if app.tutor_pulling {
        right.push(Span::styled(format!("{spin} downloading  "), st(BLUE)));
    }
    if let Some(job) = &app.chats.job {
        right.push(Span::styled(format!("{spin} book {}  ", job.status), st(BLUE)));
    }
    if app.chats.listing {
        right.push(Span::styled(format!("{spin} digi4school  "), st(BLUE)));
    }
    if app.briefing_busy || app.chat_busy || app.tutor_busy || app.chats.busy {
        right.push(Span::styled(format!("{spin} claude  "), st(ORANGE)));
    }
    let now = now_ts();
    let open: Vec<&Task> = app.tasks.iter().filter(|t| !t.done).collect();
    let overdue = open.iter().filter(|t| t.due.is_some_and(|d| d < now)).count();
    let due_today = open.iter().filter(|t| t.due.is_some_and(|d| d >= now && date_of(d) == today())).count();
    if overdue > 0 {
        right.push(Span::styled(format!("{overdue} overdue  "), st(RED)));
    }
    if due_today > 0 {
        right.push(Span::styled(format!("{due_today} today  "), st(YELLOW)));
    }
    let synced = app.last_sync.map_or("never synced".to_string(), |t| format!("synced {}", fmt_rel(t)));
    right.push(Span::styled(format!("{synced}  "), st(DIM)));
    right.push(Span::styled(format!("{} ", Local::now().format("%a %d %b %H:%M")), st(MUTED)));

    let right_width: usize = right.iter().map(|s| s.content.width()).sum();
    let right_width = (right_width as u16).min(area.width);
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);
    let bar = Style::default().bg(PANEL);
    f.render_widget(Paragraph::new(Line::from(left)).style(bar), l);
    f.render_widget(Paragraph::new(Line::from(right)).style(bar), r);
}

fn prompt_label(p: &Prompt) -> &'static str {
    match p {
        Prompt::Command => ":",
        Prompt::Search => "/",
        Prompt::NewTask(_) => "new task (title @fri 18:00 !! #tag ~30m): ",
        Prompt::EditTitle(_) => "title: ",
        Prompt::SetDue(_) => "due (fri, tom 18:00, +3d, 14.11. — empty clears): ",
        Prompt::NewNote => "create (name, or name/ for a folder): ",
        Prompt::Rename(_) => "rename: ",
        Prompt::VaultFind => "find in vault (↑ ↓ pick, Enter opens): ",
        Prompt::Setting(_) => "value: ",
        Prompt::NewHabit => "new habit: ",
        Prompt::Chat => "ask claude: ",
        Prompt::TutorAsk => "ask your tutor: ",
        Prompt::ChatTitle(_) => "chat name (optional): ",
        Prompt::ChatAsk => "ask claude: ",
        Prompt::BooksEmail => "digi4school email: ",
        Prompt::BooksPassword => "digi4school password (not saved): ",
    }
}

fn draw_cmdline(f: &mut Frame, app: &App, area: Rect) {
    match &app.mode {
        Mode::Input(p) => {
            let label = prompt_label(p);
            // A password shows as dots, and only as many as you've typed.
            let text = if *p == Prompt::BooksPassword { "•".repeat(app.input.len()) } else { app.input_text() };
            let color = if *p == Prompt::Command || *p == Prompt::Search { FG } else { GREEN };
            f.render_widget(Paragraph::new(Line::from(vec![Span::styled(label, st(color)), Span::styled(text, st(FG))])), area);
            let before: String = if *p == Prompt::BooksPassword { "•".repeat(app.cursor) } else { app.input[..app.cursor].iter().collect() };
            let x = area.x + (label.width() + before.width()).min(area.width.saturating_sub(1) as usize) as u16;
            f.set_cursor_position((x, area.y));
        }
        Mode::Confirm(c) => {
            let q = match c {
                Confirm::TrashNote(p) => format!("Move {} to the vault's .trash? (y/n)", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                Confirm::DeleteHabit(_, n) => format!("Delete habit {n} and its history? (y/n)"),
                Confirm::DeleteChat(_, title) => format!("Delete the chat {title} and its messages? Files stay. (y/n)"),
                Confirm::DeleteBook(p) => format!("Delete {} from this computer? (y/n)", p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                Confirm::DeleteTutorNote(p) => format!("Delete the note {}? (y/n)", p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                Confirm::RestartLesson(_) => "Start this lesson over and erase its conversation? Claude's notes stay. (y/n)".to_string(),
                Confirm::DeleteAgenda(title) => format!("Delete {title}? (y or d to confirm, u undoes it later)"),
                Confirm::Quit => "A focus session is running and will be lost. Quit? (y/n)".to_string(),
            };
            f.render_widget(Paragraph::new(Span::styled(q, st(YELLOW))), area);
        }
        Mode::Form => {
            f.render_widget(Paragraph::new(Span::styled("Filling in a form", st(DIM))), area);
        }
        Mode::Normal => {
            let color = if app.msg_kind == MsgKind::Error { RED } else { MUTED };
            let [m, keys] = Layout::horizontal([Constraint::Min(0), Constraint::Length(10)]).areas(area);
            f.render_widget(Paragraph::new(Span::styled(fit(&app.msg, m.width as usize), st(color))), m);
            let mut typed = String::new();
            if let Some(n) = app.count {
                typed += &n.to_string();
            }
            if let Some(p) = app.pending {
                typed.push(if p == ' ' { '␣' } else { p });
            }
            f.render_widget(Paragraph::new(Span::styled(format!("{typed:>9} "), st(DIM))), keys);
        }
    }
}

// ---- list rendering -------------------------------------------------------

enum Row<'a> {
    Header(Line<'a>),
    Item(Line<'a>),
}

/// A vim buffer: relative line numbers in the gutter and `~` after the last line.
fn draw_buffer(f: &mut Frame, area: Rect, rows: Vec<Row>, selected: usize, active: bool) {
    let height = area.height as usize;
    let mut item = 0usize;
    let mut sel_line = 0usize;
    for (i, row) in rows.iter().enumerate() {
        if let Row::Item(_) = row {
            if item == selected {
                sel_line = i;
            }
            item += 1;
        }
    }
    let scrolloff = 3.min(height / 2);
    let offset = (sel_line + scrolloff + 1).saturating_sub(height).min(rows.len().saturating_sub(height));
    // A heading just above the selection stays in view when scrolling back to the top.
    let offset = if sel_line <= scrolloff { 0 } else { offset };

    let mut item = rows.iter().take(offset).filter(|r| matches!(r, Row::Item(_))).count();
    let mut y = area.y;
    for row in rows.into_iter().skip(offset).take(height) {
        let line_area = Rect { x: area.x, y, width: area.width, height: 1 };
        match row {
            Row::Header(line) => {
                f.render_widget(Paragraph::new(line), line_area);
            }
            Row::Item(line) => {
                let is_sel = active && item == selected;
                let num = if is_sel { format!("{:>3} ", item + 1) } else { format!("{:>3} ", item.abs_diff(selected)) };
                let gutter = if is_sel { st(ORANGE).add_modifier(Modifier::BOLD) } else { st(DIM) };
                let mut spans = vec![Span::styled(num, gutter)];
                spans.extend(line.spans);
                let bg = if is_sel { Style::default().bg(SEL) } else { Style::default() };
                f.render_widget(Paragraph::new(Line::from(spans)).style(bg), line_area);
                item += 1;
            }
        }
        y += 1;
    }
    while y < area.y + area.height {
        f.render_widget(Paragraph::new(Span::styled("  ~", st(BORDER))), Rect { x: area.x, y, width: area.width, height: 1 });
        y += 1;
    }
}

fn due_color(due: i64, now: i64) -> Color {
    if due < now {
        RED
    } else if date_of(due) == today() {
        YELLOW
    } else if date_of(due) <= today() + Duration::days(2) {
        ORANGE
    } else {
        MUTED
    }
}

fn task_line<'a>(t: &Task, width: usize) -> Line<'a> {
    let now = now_ts();
    let check = if t.done { "[x] " } else { "[ ] " };
    let marks = match t.priority {
        3 => "!!!",
        2 => "!! ",
        1 => "!  ",
        _ => "   ",
    };
    let tag = course_tag(&t.course);
    let due = t.due.map_or(String::new(), fmt_rel);
    // gutter 4, check 4, marks 4, tag 9, due 9
    let title_w = width.saturating_sub(4 + 4 + 4 + 9 + 9).max(8);
    let title_style = if t.done { st(DIM).add_modifier(Modifier::CROSSED_OUT) } else { st(FG) };
    let check_color = if t.done { GREEN } else if t.is_moodle() { BLUE } else { MUTED };
    Line::from(vec![
        Span::styled(check, st(check_color)),
        Span::styled(format!("{marks} "), st(ORANGE).add_modifier(Modifier::BOLD)),
        Span::styled(pad(&t.title, title_w), title_style),
        Span::styled(format!(" {:<8}", tag), st(if t.is_moodle() { BLUE } else { DIM })),
        Span::styled(format!("{due:>9}"), st(t.due.map_or(DIM, |d| if t.done { DIM } else { due_color(d, now) }))),
    ])
}

fn section_of(t: &Task) -> &'static str {
    if t.done {
        return "Finished";
    }
    let Some(d) = t.due else { return "No date" };
    let now = now_ts();
    if d < now {
        "Overdue"
    } else if date_of(d) == today() {
        "Today"
    } else if date_of(d) == today() + Duration::days(1) {
        "Tomorrow"
    } else if date_of(d) <= today() + Duration::days(7) {
        "This week"
    } else {
        "Later"
    }
}

fn section_header<'a>(name: &str) -> Row<'a> {
    let color = match name {
        "Overdue" => RED,
        "Today" => YELLOW,
        "Finished" | "No date" => DIM,
        _ => ORANGE,
    };
    Row::Header(Line::from(Span::styled(format!("    {name}"), st(color).add_modifier(Modifier::BOLD))))
}

fn task_rows<'a>(tasks: &[&Task], width: usize) -> Vec<Row<'a>> {
    let mut rows = Vec::new();
    let mut last = "";
    for t in tasks {
        let section = section_of(t);
        if section != last {
            rows.push(section_header(section));
            last = section;
        }
        rows.push(Row::Item(task_line(t, width)));
    }
    rows
}

fn empty(f: &mut Frame, area: Rect, text: &str) {
    let inner = Rect { x: area.x + 4, y: area.y + 1, width: area.width.saturating_sub(4), height: area.height.saturating_sub(1).min(4) };
    draw_buffer(f, area, vec![], 0, false);
    f.render_widget(Clear, inner);
    f.render_widget(Paragraph::new(Span::styled(text.to_string(), st(MUTED))).wrap(Wrap { trim: false }).style(Style::default().bg(BG)), inner);
}

/// Inline Markdown: **bold**, `code` and [[wikilinks]].
fn inline_spans<'a>(text: &str, base: Style) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let mut rest = text;
    loop {
        let next = [("**", "**"), ("`", "`"), ("[[", "]]")]
            .iter()
            .filter_map(|(open, close)| {
                let start = rest.find(open)?;
                let end = rest[start + open.len()..].find(close)?;
                Some((start, end, *open, *close))
            })
            .min_by_key(|(start, ..)| *start);
        let Some((start, end, open, close)) = next else {
            if !rest.is_empty() {
                spans.push(Span::styled(rest.to_string(), base));
            }
            return spans;
        };
        if start > 0 {
            spans.push(Span::styled(rest[..start].to_string(), base));
        }
        let inner = &rest[start + open.len()..start + open.len() + end];
        let style = match open {
            "**" => base.add_modifier(Modifier::BOLD),
            "`" => st(YELLOW).bg(PANEL),
            _ => st(BLUE).add_modifier(Modifier::UNDERLINED),
        };
        // [[Page|shown text]] displays only the part after the bar.
        let shown = if open == "[[" { inner.rsplit('|').next().unwrap_or(inner) } else { inner };
        spans.push(Span::styled(shown.to_string(), style));
        rest = &rest[start + open.len() + end + close.len()..];
    }
}

/// A small Markdown renderer for previews, briefings and chat.
fn md_lines<'a>(text: &str) -> Vec<Line<'a>> {
    let mut out = Vec::new();
    let mut code = false;
    for raw in text.lines() {
        if raw.trim_start().starts_with("```") {
            code = !code;
            continue;
        }
        if code {
            out.push(Line::from(Span::styled(format!("  {raw}"), st(YELLOW))));
            continue;
        }
        let trimmed = raw.trim_start();
        if trimmed.len() >= 3 && trimmed.chars().all(|c| c == '-') {
            out.push(Line::from(Span::styled("─".repeat(32), st(BORDER))));
            continue;
        }
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            let after = &trimmed[level..];
            if after.is_empty() || after.starts_with(' ') {
                let style = match level {
                    1 => st(ORANGE).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                    2 => st(ORANGE).add_modifier(Modifier::BOLD),
                    _ => st(YELLOW).add_modifier(Modifier::BOLD),
                };
                out.push(Line::from(Span::styled(after.trim().to_string(), style)));
                continue;
            }
        }
        let indent = raw.len() - trimmed.len();
        let mut spans = vec![Span::raw(" ".repeat(indent))];
        let mut base = st(FG);
        let body = if let Some(b) = trimmed.strip_prefix("- [ ] ").or(trimmed.strip_prefix("* [ ] ")) {
            spans.push(Span::styled("☐ ", st(MUTED)));
            b
        } else if let Some(b) = ["- [x] ", "- [X] ", "* [x] ", "* [X] "].iter().find_map(|p| trimmed.strip_prefix(p)) {
            spans.push(Span::styled("☑ ", st(GREEN)));
            base = st(DIM);
            b
        } else if let Some(b) = trimmed.strip_prefix("- ").or(trimmed.strip_prefix("* ")) {
            spans.push(Span::styled("• ", st(ORANGE)));
            b
        } else if let Some(b) = trimmed.strip_prefix("> ") {
            spans.push(Span::styled("▌ ", st(ORANGE)));
            base = st(MUTED);
            b
        } else {
            trimmed
        };
        spans.extend(inline_spans(body, base));
        out.push(Line::from(spans));
    }
    out
}

fn with_preview(area: Rect) -> (Rect, Option<Rect>) {
    if area.width >= 100 {
        let [a, b] = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(area);
        (a, Some(b))
    } else {
        (area, None)
    }
}

fn task_detail<'a>(t: &Task) -> Vec<Line<'a>> {
    let mut l = vec![Line::from(Span::styled(t.title.clone(), st(FG).add_modifier(Modifier::BOLD)))];
    let label = |name: &str, value: String, color: Color| {
        Line::from(vec![Span::styled(format!("{name:<8}"), st(DIM)), Span::styled(value, st(color))])
    };
    if !t.course.is_empty() {
        l.push(label("course", t.course.clone(), BLUE));
    }
    if let Some(d) = t.due {
        l.push(label("due", format!("{} ({})", fmt_abs(d), fmt_rel(d)), due_color(d, now_ts())));
    }
    if let Some(e) = t.effort_min {
        l.push(label("effort", format!("about {e} min"), MUTED));
    }
    if !t.action.is_empty() {
        l.push(label("moodle", t.action.clone(), MUTED));
    }
    if t.done {
        let by = if t.done_by.as_deref() == Some("moodle") { "gone from Moodle's timeline" } else { "marked by you" };
        l.push(label("done", by.to_string(), GREEN));
    }
    if !t.ai_note.is_empty() {
        l.push(Line::default());
        l.push(Line::from(vec![Span::styled("✻ ", st(ORANGE)), Span::styled(t.ai_note.clone(), st(FG))]));
    }
    if !t.notes.is_empty() {
        l.push(Line::default());
        l.extend(md_lines(&t.notes));
    }
    l
}

// ---- views ----------------------------------------------------------------

fn draw_today(f: &mut Frame, app: &App, area: Rect) {
    let wide = area.width >= 100;
    let [left, right] = if wide {
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area)
    } else {
        Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area)
    };

    let mut brief: Vec<Line> = Vec::new();
    match &app.briefing {
        Some(b) => {
            brief.push(Line::from(Span::styled(b.headline.clone(), st(ORANGE).add_modifier(Modifier::BOLD))));
            brief.push(Line::default());
            brief.extend(md_lines(&b.plan));
            brief.push(Line::default());
            brief.push(Line::from(Span::styled(format!("written {}", fmt_rel(b.created_at)), st(DIM))));
        }
        None => {
            brief.push(Line::from(Span::styled("No briefing yet.", st(MUTED))));
            brief.push(Line::from(Span::styled("Press Space then b, or wait for the 15:00 run.", st(DIM))));
        }
    }
    if !app.inbox.is_empty() {
        brief.push(Line::default());
        brief.push(Line::from(Span::styled(format!("Moodle inbox ({} unread)", app.inbox.len()), st(BLUE).add_modifier(Modifier::BOLD))));
        for n in app.inbox.iter().take(5) {
            brief.push(Line::from(vec![Span::styled("• ", st(BLUE)), Span::styled(n.subject.clone(), st(MUTED))]));
        }
    }
    f.render_widget(Paragraph::new(brief).wrap(Wrap { trim: false }).scroll((app.preview_scroll, 0)).block(panel(" ✻ Claude's briefing ")), left);

    let today_events: Vec<_> = app.events.iter().filter(|e| date_of(e.start) == today()).collect();
    let extra = today_events.len() as u16 + app.habits.len() as u16;
    let lower = if extra > 0 { (extra + 4).min(right.height / 2) } else { 0 };
    let [list, bottom] = Layout::vertical([Constraint::Min(3), Constraint::Length(lower)]).areas(right);

    let block = panel(" Next up ");
    let inner = block.inner(list);
    f.render_widget(block, list);
    let tasks = app.today_tasks();
    if tasks.is_empty() {
        empty(f, inner, "Nothing urgent. Press o to add a task.");
    } else {
        draw_buffer(f, inner, task_rows(&tasks, inner.width as usize), app.sel_today, true);
    }

    if lower > 0 {
        let mut lines: Vec<Line> = Vec::new();
        for e in &today_events {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", local(e.start).format("%H:%M")), st(ORANGE)),
                Span::styled(e.title.clone(), st(FG)),
            ]));
        }
        if !today_events.is_empty() && !app.habits.is_empty() {
            lines.push(Line::default());
        }
        for h in &app.habits {
            let done = h.days.contains(&today());
            lines.push(Line::from(vec![
                Span::styled(if done { "● " } else { "○ " }, st(if done { GREEN } else { DIM })),
                Span::styled(h.name.clone(), st(if done { MUTED } else { FG })),
            ]));
        }
        f.render_widget(Paragraph::new(lines).block(panel(" Today ")), bottom);
    }
}

fn draw_tasks(f: &mut Frame, app: &App, area: Rect) {
    let (list, preview) = with_preview(area);
    let tasks = app.visible_tasks();
    if tasks.is_empty() {
        empty(f, list, "No tasks here. Press o to add one, or gs to sync Moodle.");
    } else {
        draw_buffer(f, list, task_rows(&tasks, list.width as usize), app.sel_tasks, true);
    }
    if let Some(p) = preview {
        let lines = tasks.get(app.sel_tasks).map(|t| task_detail(t)).unwrap_or_default();
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((app.preview_scroll, 0)).block(panel(" Details ")), p);
    }
}

fn draw_calendar(f: &mut Frame, app: &App, area: Rect) {
    let wide = area.width >= 90;
    let [grid_area, side] = if wide {
        Layout::horizontal([Constraint::Min(50), Constraint::Length(38)]).areas(area)
    } else {
        Layout::vertical([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(area)
    };
    let title = format!(" {} ", app.cal_date.format("%B %Y"));
    let block = panel(title);
    let inner = block.inner(grid_area);
    f.render_widget(block, grid_area);
    if inner.height < 3 || inner.width < 14 {
        return;
    }

    let cell_w = inner.width / 7;
    let cell_h = ((inner.height - 1) / 6).max(1);
    for (i, name) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].iter().enumerate() {
        let r = Rect { x: inner.x + i as u16 * cell_w, y: inner.y, width: cell_w, height: 1 };
        f.render_widget(Paragraph::new(Span::styled(*name, st(DIM))), r);
    }
    let first = month_first_cell(app.cal_date);
    for week in 0..6u16 {
        for dow in 0..7u16 {
            let day = first + Duration::days((week * 7 + dow) as i64);
            let rect = Rect { x: inner.x + dow * cell_w, y: inner.y + 1 + week * cell_h, width: cell_w.saturating_sub(1), height: cell_h };
            if rect.y + rect.height > inner.y + inner.height {
                continue;
            }
            let same_month = day.month() == app.cal_date.month();
            let items = app.agenda(day);
            let is_sel = day == app.cal_date;
            let is_today = day == today();
            let bg = if is_sel { SEL } else { BG };
            let num_style = if is_today {
                Style::default().fg(BG).bg(ORANGE).add_modifier(Modifier::BOLD)
            } else if is_sel {
                st(FG).add_modifier(Modifier::BOLD)
            } else if same_month {
                st(MUTED)
            } else {
                st(BORDER)
            };
            let mut first_line = vec![Span::styled(format!("{:>2}", day.day()), num_style)];
            if cell_h == 1 && !items.is_empty() {
                first_line.push(Span::styled(format!(" {}", items.len().min(9)), st(ORANGE)));
            }
            let mut lines = vec![Line::from(first_line)];
            let slots = rect.height.saturating_sub(1) as usize;
            for (i, item) in items.iter().enumerate() {
                if i >= slots {
                    break;
                }
                if i + 1 == slots && items.len() > slots {
                    lines.push(Line::from(Span::styled(format!("+{}", items.len() - i), st(DIM))));
                    break;
                }
                let color = match item {
                    AgendaItem::Task(t) if t.is_moodle() => BLUE,
                    AgendaItem::Task(_) => ORANGE,
                    AgendaItem::Event(_) => GREEN,
                };
                let color = if same_month { color } else { DIM };
                lines.push(Line::from(Span::styled(fit(item.title(), rect.width as usize), st(color))));
            }
            f.render_widget(Paragraph::new(lines).style(Style::default().bg(bg)), rect);
        }
    }

    draw_agenda(f, app, side);
}

fn draw_agenda(f: &mut Frame, app: &App, area: Rect) {
    let items = app.agenda(app.cal_date);
    let hints: [(&str, &str); 5] = [("o", "add event"), ("a", "add task"), ("Enter", "edit"), ("d", "delete"), ("+ -", "move a day")];
    let block = panel(format!(" {} ", app.cal_date.format("%A %d %B")));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let hint_h = if inner.height >= 12 { hints.len() as u16 + 1 } else { 0 };
    let [list_area, hint_area] = Layout::vertical([Constraint::Min(0), Constraint::Length(hint_h)]).areas(inner);

    let mut lines: Vec<Line> = Vec::new();
    if items.is_empty() {
        lines.push(Line::from(Span::styled("Nothing scheduled.", st(DIM))));
        lines.push(Line::from(Span::styled("Press Enter to add something.", st(MUTED))));
    }
    for (i, item) in items.iter().enumerate() {
        let is_sel = i == app.sel_agenda;
        let (color, from_moodle) = match item {
            AgendaItem::Task(t) => (if t.is_moodle() { BLUE } else { ORANGE }, t.is_moodle()),
            AgendaItem::Event(e) => (GREEN, e.source == "moodle"),
        };
        let when = match item {
            AgendaItem::Event(e) if e.end > e.start => format!("{}–{}", local(e.start).format("%H:%M"), local(e.end).format("%H:%M")),
            _ => local(item.time()).format("%H:%M").to_string(),
        };
        let bg = if is_sel { Style::default().bg(SEL) } else { Style::default() };
        lines.push(
            Line::from(vec![
                Span::styled(if is_sel { "› " } else { "  " }, st(ORANGE)),
                Span::styled(format!("{when} "), st(color)),
                Span::styled(item.title().to_string(), st(FG).add_modifier(if is_sel { Modifier::BOLD } else { Modifier::empty() })),
                Span::styled(if from_moodle { " moodle" } else { "" }, st(DIM)),
            ])
            .style(bg),
        );
        if is_sel {
            let (location, notes) = match item {
                AgendaItem::Event(e) => (e.location.as_str(), e.notes.as_str()),
                AgendaItem::Task(_) => ("", ""),
            };
            if !location.is_empty() {
                lines.push(Line::from(Span::styled(format!("    @ {location}"), st(MUTED))));
            }
            if !notes.is_empty() {
                lines.push(Line::from(Span::styled(format!("    {notes}"), st(MUTED))));
            }
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), list_area);

    if hint_h > 0 {
        let mut text = vec![Line::default()];
        text.extend(hints.iter().map(|(k, d)| Line::from(vec![Span::styled(format!("{k:<6}"), st(ORANGE)), Span::styled(*d, st(DIM))])));
        f.render_widget(Paragraph::new(text), hint_area);
    }
}

fn draw_form(f: &mut Frame, app: &App, body: Rect) {
    let Some(form) = &app.form else { return };
    let height = (form.fields.len() as u16 + 7).min(body.height);
    let width = 62.min(body.width);
    let rect = Rect { x: body.x + (body.width - width) / 2, y: body.y + (body.height - height) / 2, width, height };
    let title = match form.target {
        Target::Event(None) => " New event ",
        Target::Task(None) => " New task ",
        Target::Event(Some(_)) => " Edit event ",
        Target::Task(Some(_)) => " Edit task ",
    };
    let block = panel(title);
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block.style(Style::default().bg(PANEL)), rect);
    if inner.height == 0 {
        return;
    }
    const LABEL: u16 = 11;
    let room = inner.width.saturating_sub(LABEL + 1) as usize;
    let mut y = inner.y;
    for (i, field) in form.fields.iter().enumerate() {
        if y >= inner.y + inner.height {
            break;
        }
        let focused = i == form.focus;
        let text: String = field.value.iter().collect();
        let value_style = if focused && form.fresh {
            Style::default().fg(BG).bg(ORANGE)
        } else if focused {
            st(FG).bg(SEL)
        } else {
            st(FG)
        };
        let shown = if focused { pad(&text, room) } else { fit(&text, room) };
        let line = if focused && form.fresh {
            Line::from(vec![
                Span::styled(format!("{:<11}", field.label), st(ORANGE).add_modifier(Modifier::BOLD)),
                Span::styled(fit(&text, room), value_style),
                Span::styled(" ".repeat(room.saturating_sub(fit(&text, room).width())), st(FG).bg(SEL)),
            ])
        } else {
            Line::from(vec![Span::styled(format!("{:<11}", field.label), if focused { st(ORANGE).add_modifier(Modifier::BOLD) } else { st(MUTED) }), Span::styled(shown, value_style)])
        };
        f.render_widget(Paragraph::new(line), Rect { x: inner.x, y, width: inner.width, height: 1 });
        if focused && !form.fresh {
            let before: String = field.value[..form.cursor.min(field.value.len())].iter().collect();
            let x = inner.x + LABEL + (before.width() as u16).min(room as u16);
            f.set_cursor_position((x.min(inner.x + inner.width.saturating_sub(1)), y));
        }
        y += 1;
    }

    let note = if let Some(err) = &form.error {
        Line::from(Span::styled(err.clone(), st(RED)))
    } else if form.focus == crate::form::DATE {
        match form.date_preview() {
            Some(d) => Line::from(Span::styled(d, st(GREEN))),
            None => Line::from(Span::styled("Try fri, tom, +3d, 14.11. or 14.11.2026", st(YELLOW))),
        }
    } else {
        let hint = match form.focus {
            crate::form::TITLE => "What is it?",
            crate::form::TIME => "Like 14:00 or 9",
            crate::form::LENGTH => "Like 45m, 1h or 1.5h",
            _ => "Optional",
        };
        Line::from(Span::styled(hint, st(DIM)))
    };
    let keys = if form.target.is_new() { "Tab next field  Enter save  Esc cancel  Ctrl-t event/task" } else { "Tab next field  Enter save  Esc cancel" };
    let bottom = inner.y + inner.height;
    if y + 1 < bottom {
        f.render_widget(Paragraph::new(note).wrap(Wrap { trim: false }), Rect { x: inner.x, y: y + 1, width: inner.width, height: (bottom - y - 2).min(2) });
    }
    if inner.height > 1 {
        f.render_widget(Paragraph::new(Span::styled(fit(keys, inner.width as usize), st(DIM))), Rect { x: inner.x, y: bottom - 1, width: inner.width, height: 1 });
    }
}

fn file_icon(e: &Entry, nerd: bool) -> (&'static str, Color) {
    if e.is_dir {
        return (if nerd { "\u{f07b}" } else { "▸" }, BLUE);
    }
    let ext = e.path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
    let (icon, color) = match ext.as_str() {
        "md" | "markdown" => ("\u{f48a}", ORANGE),
        "pdf" => ("\u{f1c1}", RED),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp" => ("\u{f1c5}", GREEN),
        "mp4" | "mkv" | "webm" | "mov" => ("\u{f1c8}", GREEN),
        "mp3" | "wav" | "flac" | "ogg" => ("\u{f1c7}", GREEN),
        "zip" | "tar" | "gz" | "7z" | "rar" | "xz" => ("\u{f1c6}", YELLOW),
        "doc" | "docx" | "odt" => ("\u{f1c2}", BLUE),
        "xls" | "xlsx" | "csv" | "ods" => ("\u{f1c3}", GREEN),
        "ppt" | "pptx" | "odp" => ("\u{f1c4}", ORANGE),
        "dll" | "so" | "exe" | "bin" | "o" | "cache" => ("\u{f013}", DIM),
        "json" | "toml" | "yaml" | "yml" | "xml" | "ini" | "conf" => ("\u{f013}", YELLOW),
        "rs" | "py" | "c" | "h" | "cc" | "cpp" | "cs" | "java" | "js" | "ts" | "html" | "css" | "sh" | "lua" | "go" => ("\u{f121}", BLUE),
        "txt" | "log" => ("\u{f15c}", MUTED),
        _ => ("\u{f15b}", MUTED),
    };
    (if nerd { icon } else { "·" }, color)
}

/// One Yazi column. The focused selection gets a full terracotta bar and others a quiet one.
fn draw_entries(f: &mut Frame, area: Rect, entries: &[&Entry], sel: Option<usize>, focused: bool, app: &App, sizes: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if entries.is_empty() {
        f.render_widget(Paragraph::new(Span::styled(" empty", st(DIM))), Rect { height: 1, ..area });
        return;
    }
    let height = area.height as usize;
    let at = sel.unwrap_or(0);
    let scrolloff = 3.min(height / 2);
    let offset = (at + scrolloff + 1).saturating_sub(height).min(entries.len().saturating_sub(height));
    let width = area.width as usize;
    for (row, (i, e)) in entries.iter().enumerate().skip(offset).take(height).enumerate() {
        let (icon, icon_color) = file_icon(e, app.cfg.nerd_icons);
        let size = if !sizes || e.is_dir { String::new() } else { human_size(e.size) };
        let name_w = width.saturating_sub(3 + size.width() + 2);
        let name = pad(&e.name, name_w);
        let is_sel = sel == Some(i);
        let yanked = app.browser.yank.as_ref().filter(|y| y.path == e.path);
        let name_color = match yanked {
            Some(y) if y.cut => RED,
            Some(_) => GREEN,
            None if e.is_dir => BLUE,
            None => FG,
        };
        let mut name_style = st(name_color);
        if e.is_dir {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        let line = if is_sel && focused {
            let bar = Style::default().fg(BG).bg(ORANGE).add_modifier(Modifier::BOLD);
            Line::from(vec![Span::styled(format!(" {icon} {name}  {size}"), bar)])
        } else {
            let bg = if is_sel { SEL } else { BG };
            Line::from(vec![
                Span::styled(format!(" {icon} "), st(icon_color).bg(bg)),
                Span::styled(name, name_style.bg(bg)),
                Span::styled(format!("  {size}"), st(DIM).bg(bg)),
            ])
        };
        let rect = Rect { x: area.x, y: area.y + row as u16, width: area.width, height: 1 };
        f.render_widget(Paragraph::new(line).style(Style::default().bg(if is_sel && focused { ORANGE } else if is_sel { SEL } else { BG })), rect);
    }
}

fn draw_find_results(f: &mut Frame, area: Rect, app: &App) {
    let matches = app.find_matches();
    if matches.is_empty() {
        f.render_widget(Paragraph::new(Span::styled(" no matching notes", st(DIM))), Rect { height: 1, ..area });
        return;
    }
    let height = area.height as usize;
    let offset = (app.find_sel + 1).saturating_sub(height);
    for (row, (i, n)) in matches.iter().enumerate().skip(offset).take(height).enumerate() {
        let rect = Rect { x: area.x, y: area.y + row as u16, width: area.width, height: 1 };
        let (dir, name) = n.rel.rsplit_once('/').map_or(("", n.rel.as_str()), |(d, b)| (d, b));
        let dir = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        let room = (area.width as usize).saturating_sub(1 + dir.width());
        let line = if i == app.find_sel {
            let bar = Style::default().fg(BG).bg(ORANGE).add_modifier(Modifier::BOLD);
            Line::from(Span::styled(format!(" {dir}{}", fit(name, room)), bar)).style(bar)
        } else {
            Line::from(vec![Span::raw(" "), Span::styled(dir, st(DIM)), Span::styled(fit(name, room), st(FG))])
        };
        f.render_widget(Paragraph::new(line), rect);
    }
}

fn draw_file_preview(f: &mut Frame, area: Rect, app: &App, target: Option<&Entry>) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let inner = Rect { x: area.x + 1, width: area.width.saturating_sub(2), ..area };
    draw_preview_content(f, inner, &app.preview, target, app);
}

fn draw_preview_content(f: &mut Frame, inner: Rect, preview: &Preview, target: Option<&Entry>, app: &App) {
    let area = inner;
    match preview {
        Preview::None => {}
        Preview::Dir(children) => {
            let refs: Vec<&Entry> = children.iter().collect();
            draw_entries(f, area, &refs, None, false, app, true);
        }
        Preview::Markdown(text) => {
            f.render_widget(Paragraph::new(md_lines(text)).wrap(Wrap { trim: false }).scroll((app.preview_scroll, 0)), inner);
        }
        Preview::Text(text) => {
            let lines: Vec<Line> = text
                .lines()
                .enumerate()
                .map(|(i, l)| Line::from(vec![Span::styled(format!("{:>4} ", i + 1), st(BORDER)), Span::styled(l.replace('\t', "    "), st(MUTED))]))
                .collect();
            f.render_widget(Paragraph::new(lines).scroll((app.preview_scroll, 0)), inner);
        }
        Preview::Pdf { pages, text } => {
            let count = if *pages == 1 { "1 page".to_string() } else { format!("{pages} pages") };
            let mut lines = vec![
                Line::from(vec![Span::styled(format!("PDF · {count}"), st(RED).add_modifier(Modifier::BOLD)), Span::styled("   Enter reads it here", st(DIM))]),
                Line::default(),
            ];
            if text.trim().is_empty() {
                lines.push(Line::from(Span::styled("No text on the first page. It may be a scan, which still opens fine.", st(DIM))));
            }
            lines.extend(text.lines().map(|l| Line::from(Span::styled(l.trim_start().to_string(), st(MUTED)))));
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((app.preview_scroll, 0)), inner);
        }
        Preview::Binary => {
            let top = area.height.saturating_sub(5) / 2;
            let mut lines: Vec<Line> = (0..top).map(|_| Line::default()).collect();
            if let Some(e) = target {
                let (icon, color) = file_icon(e, app.cfg.nerd_icons);
                lines.push(Line::from(Span::styled(icon, st(color).add_modifier(Modifier::BOLD))).centered());
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(fit(&e.name, inner.width as usize), st(FG))).centered());
                lines.push(Line::from(Span::styled(human_size(e.size), st(DIM))).centered());
                lines.push(Line::from(Span::styled("Enter opens it in your default app", st(DIM))).centered());
            }
            f.render_widget(Paragraph::new(lines), inner);
        }
    }
}

fn draw_notes(f: &mut Frame, app: &App, area: Rect) {
    if area.height < 3 {
        return;
    }
    let [header, columns, footer] = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(area);

    // The path, with the current folder picked out.
    let location = app.browser.location();
    let (dir, leaf) = location.rsplit_once('/').map_or(("", location.as_str()), |(d, l)| (d, l));
    let mut head = vec![Span::raw(" ")];
    if !dir.is_empty() {
        head.push(Span::styled(format!("{dir}/"), st(MUTED)));
    }
    head.push(Span::styled(leaf.to_string(), st(BLUE).add_modifier(Modifier::BOLD)));
    if !app.search.is_empty() {
        head.push(Span::styled(format!("  filter: {}", app.search), st(YELLOW)));
    }
    f.render_widget(Paragraph::new(Line::from(head)), header);

    // Yazi's 1:4:3 proportions, dropping the outer columns when space runs short.
    let w = columns.width;
    let (parent, current, preview) = if w >= 80 {
        let [a, b, c] = Layout::horizontal([Constraint::Ratio(1, 8), Constraint::Ratio(4, 8), Constraint::Ratio(3, 8)]).areas(columns);
        (Some(a), b, Some(c))
    } else if w >= 45 {
        let [b, c] = Layout::horizontal([Constraint::Ratio(5, 8), Constraint::Ratio(3, 8)]).areas(columns);
        (None, b, Some(c))
    } else {
        (None, columns, None)
    };
    let divider = Block::default().borders(Borders::RIGHT).border_style(st(BORDER));

    if let Some(p) = parent {
        let inner = divider.clone().inner(p);
        f.render_widget(divider.clone(), p);
        let refs: Vec<&Entry> = app.browser.parent.iter().collect();
        if app.browser.parent_dir().is_some() {
            let at = app.browser.cwd.file_name().and_then(|n| refs.iter().position(|e| e.name == n.to_string_lossy()));
            draw_entries(f, inner, &refs, at, false, app, false);
        }
    }

    let finding = app.is_finding();
    let visible = app.visible_entries();
    let current_inner = if preview.is_some() { divider.clone().inner(current) } else { current };
    if preview.is_some() {
        f.render_widget(divider, current);
    }
    if finding {
        draw_find_results(f, current_inner, app);
    } else if !std::path::Path::new(&app.cfg.vault).is_dir() {
        f.render_widget(
            Paragraph::new(Span::styled(format!(" Vault not found: {}", app.cfg.vault), st(RED))).wrap(Wrap { trim: false }),
            current_inner,
        );
    } else {
        draw_entries(f, current_inner, &visible, Some(app.sel_notes), true, app, true);
    }
    let target = visible.get(app.sel_notes).copied();
    if let Some(p) = preview {
        draw_file_preview(f, p, app, target);
    }

    // A status strip like Yazi's: position and file details on the left, modes on the right.
    let mut left = vec![Span::styled(format!(" {}/{} ", (app.sel_notes + 1).min(visible.len()), visible.len()), Style::default().fg(BG).bg(BLUE))];
    if let Some(e) = target {
        if !e.is_dir {
            left.push(Span::styled(format!(" {}", human_size(e.size)), st(MUTED)));
        }
        left.push(Span::styled(format!("  edited {}", fmt_rel(e.modified)), st(DIM)));
    }
    if let Some(y) = &app.browser.yank {
        let name = y.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let (verb, color) = if y.cut { ("cut", RED) } else { ("copied", GREEN) };
        left.push(Span::styled(format!("  {verb}: {name}"), st(color)));
    }
    let sort = match app.browser.sort {
        Sort::Name => "name",
        Sort::Modified => "modified",
        Sort::Size => "size",
    };
    let hidden = if app.browser.show_hidden { "  hidden shown" } else { "" };
    let right = format!("sort: {sort}{hidden} ");
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right.width() as u16)]).areas(footer);
    f.render_widget(Paragraph::new(Line::from(left)), l);
    f.render_widget(Paragraph::new(Span::styled(right, st(DIM))), r);
}

fn draw_settings(f: &mut Frame, app: &App, area: Rect) {
    let (list, side) = with_preview(area);
    let label_w = 28usize;
    let value_w = (list.width as usize).saturating_sub(4 + label_w + 1);
    let mut rows = Vec::new();
    let mut last = "";
    for (section, key) in settings::ROWS {
        if *section != last {
            rows.push(Row::Header(Line::from(Span::styled(format!("    {section}"), st(ORANGE).add_modifier(Modifier::BOLD)))));
            last = section;
        }
        let value = settings::value(&app.cfg, *key);
        let color = match (settings::kind(*key), value.as_str()) {
            (Kind::Toggle, "on") => GREEN,
            (Kind::Toggle, _) => DIM,
            (Kind::Action, _) => DIM,
            (Kind::Choice, _) => YELLOW,
            (Kind::Text, _) => FG,
        };
        rows.push(Row::Item(Line::from(vec![Span::styled(pad(settings::label(*key), label_w), st(MUTED)), Span::raw(" "), Span::styled(fit(&value, value_w), st(color))])));
    }
    draw_buffer(f, list, rows, app.sel_settings, true);

    let Some(side) = side else { return };
    let key = settings::ROWS[app.sel_settings.min(settings::ROWS.len() - 1)].1;
    let how = match settings::kind(key) {
        Kind::Toggle => "Enter or x switches it on or off",
        Kind::Choice => "h and l change it",
        Kind::Text => "Enter edits it",
        Kind::Action => "Enter runs it",
    };
    let stamp = |k: &str| app.db.meta(k).and_then(|v| v.parse::<i64>().ok()).map_or("never".to_string(), |t| format!("{} ({})", fmt_abs(t), fmt_rel(t)));
    let row = |name: &str, value: String| Line::from(vec![Span::styled(format!("{name:<11}"), st(DIM)), Span::styled(value, st(FG))]);
    let next = if app.cfg.daily_enabled { app.timer_next.clone() } else { "off".to_string() };
    let lines = vec![
        Line::from(Span::styled(settings::label(key), st(FG).add_modifier(Modifier::BOLD))),
        Line::default(),
        Line::from(Span::styled(settings::help(key), st(MUTED))),
        Line::default(),
        Line::from(Span::styled(how, st(ORANGE))),
        Line::default(),
        Line::from(Span::styled("Schedule", st(ORANGE).add_modifier(Modifier::BOLD))),
        row("next run", next),
        row("last run", stamp("last_daily")),
        row("last sync", stamp("last_sync")),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel(" About this setting ")), side);
}

fn draw_habits(f: &mut Frame, app: &App, area: Rect) {
    if app.habits.is_empty() {
        return empty(f, area, "No habits yet. Press o to add one, then x to tick it off each day.");
    }
    const NAME_W: usize = 24;
    let mut header = vec![Span::raw(" ".repeat(4 + NAME_W + 1))];
    for back in (0..14).rev() {
        let day = today() - Duration::days(back);
        let label = day.format("%a").to_string().chars().next().unwrap_or(' ');
        let style = if back == app.habit_col { st(ORANGE).add_modifier(Modifier::BOLD) } else { st(DIM) };
        header.push(Span::styled(format!("{label} "), style));
    }
    header.push(Span::styled("  streak", st(DIM)));
    let mut rows = vec![Row::Header(Line::from(header))];
    for h in &app.habits {
        let mut spans = vec![Span::styled(pad(&h.name, NAME_W), st(FG)), Span::raw(" ")];
        for back in (0..14).rev() {
            let done = h.days.contains(&(today() - Duration::days(back)));
            let mut style = st(if done { ORANGE } else { BORDER });
            if back == app.habit_col {
                style = style.add_modifier(Modifier::BOLD).bg(PANEL);
            }
            spans.push(Span::styled(if done { "● " } else { "· " }, style));
        }
        let streak = h.streak();
        spans.push(Span::styled(format!("  {streak}d"), st(if streak > 0 { ORANGE } else { DIM })));
        rows.push(Row::Item(Line::from(spans)));
    }
    draw_buffer(f, area, rows, app.sel_habits, true);
}

fn grade_detail<'a>(g: &Grade) -> Vec<Line<'a>> {
    let mut l = vec![Line::from(Span::styled(g.course.clone(), st(FG).add_modifier(Modifier::BOLD))), Line::default()];
    if g.items.is_empty() {
        let note = if g.current { "No grade items yet." } else { "Items aren't synced for past courses." };
        l.push(Line::from(Span::styled(note, st(DIM))));
    }
    for it in &g.items {
        let style = if it.total { st(ORANGE).add_modifier(Modifier::BOLD) } else { st(FG) };
        l.push(Line::from(vec![
            Span::styled(pad(&it.name, 30), style),
            Span::styled(format!(" {:>8}", it.grade), style),
            Span::styled(format!(" {:>9}", it.pct), st(DIM)),
        ]));
        if !it.feedback.is_empty() {
            l.push(Line::from(Span::styled(format!("  │ {}", it.feedback), st(MUTED))));
        }
    }
    l
}

fn draw_grades(f: &mut Frame, app: &App, area: Rect) {
    let (list, preview) = with_preview(area);
    let grades = app.visible_grades();
    if grades.is_empty() {
        empty(f, list, "No grades synced yet. Press gs to sync Moodle.");
    } else {
        let w = (list.width as usize).saturating_sub(4 + 12);
        let rows: Vec<Row> = grades
            .iter()
            .map(|g| {
                let color = if g.grade == "-" { DIM } else { ORANGE };
                Row::Item(Line::from(vec![Span::styled(format!("{:>10}  ", fit(&g.grade, 10)), st(color)), Span::styled(fit(&g.course, w), st(if g.current { FG } else { DIM }))]))
            })
            .collect();
        draw_buffer(f, list, rows, app.sel_grades, true);
    }
    if let Some(p) = preview {
        let lines = grades.get(app.sel_grades).map(|g| grade_detail(g)).unwrap_or_default();
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((app.preview_scroll, 0)).block(panel(" Items ")), p);
    }
}

const DIGITS: [[&str; 5]; 11] = [
    ["█████", "█   █", "█   █", "█   █", "█████"],
    ["  █  ", "  █  ", "  █  ", "  █  ", "  █  "],
    ["█████", "    █", "█████", "█    ", "█████"],
    ["█████", "    █", "█████", "    █", "█████"],
    ["█   █", "█   █", "█████", "    █", "    █"],
    ["█████", "█    ", "█████", "    █", "█████"],
    ["█████", "█    ", "█████", "█   █", "█████"],
    ["█████", "    █", "    █", "    █", "    █"],
    ["█████", "█   █", "█████", "█   █", "█████"],
    ["█████", "█   █", "█████", "    █", "█████"],
    ["     ", "  █  ", "     ", "  █  ", "     "],
];

fn big_time(secs: u64) -> Vec<String> {
    let text = format!("{:02}:{:02}", secs / 60, secs % 60);
    (0..5)
        .map(|row| {
            text.chars()
                .map(|c| DIGITS[if c == ':' { 10 } else { c.to_digit(10).unwrap_or(0) as usize }][row])
                .collect::<Vec<_>>()
                .join("  ")
        })
        .collect()
}

fn draw_focus(f: &mut Frame, app: &App, area: Rect) {
    let center = |text: String, color: Color| Line::from(Span::styled(text, st(color))).centered();
    let mut lines: Vec<Line> = Vec::new();
    let top = area.height.saturating_sub(13) / 2;
    lines.extend((0..top).map(|_| Line::default()));
    let (secs, color, caption) = match &app.focus {
        Some(p) => {
            let c = if p.paused { DIM } else if p.phase == Phase::Work { ORANGE } else { GREEN };
            let state = if p.paused { "paused" } else if p.phase == Phase::Work { "focusing on" } else { "break" };
            (p.remaining(), c, format!("{state}  {}", p.title))
        }
        None => (app.cfg.focus_minutes.max(0) as u64 * 60, BORDER, "not running".to_string()),
    };
    lines.push(center(caption, MUTED));
    lines.push(Line::default());
    lines.extend(big_time(secs).into_iter().map(|row| center(row, color)));
    lines.push(Line::default());
    if let Some(p) = &app.focus {
        let width = 41usize;
        let total = p.total_secs().max(1);
        let filled = ((total - p.remaining().min(total)) as usize * width) / total as usize;
        lines.push(Line::from(vec![Span::styled("█".repeat(filled), st(color)), Span::styled("░".repeat(width - filled), st(BORDER))]).centered());
    } else {
        lines.push(Line::default());
    }
    lines.push(Line::default());
    lines.push(center(format!("{} min focused today", app.focus_today), DIM));
    lines.push(Line::default());
    lines.push(center("s start   p pause   b break   x stop   + / - 5 min".to_string(), DIM));
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_claude(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if app.chat.is_empty() && !app.chat_busy {
        lines.push(Line::from(Span::styled("Ask anything about your workload. Press i to start.", st(MUTED))));
        lines.push(Line::from(Span::styled("Claude sees your tasks, calendar, habits and Moodle inbox. It never sees your notes.", st(DIM))));
    }
    for (is_user, text) in &app.chat {
        if *is_user {
            lines.push(Line::from(Span::styled("› You", st(BLUE).add_modifier(Modifier::BOLD))));
            lines.extend(text.lines().map(|l| Line::from(Span::styled(l.to_string(), st(FG)))));
        } else {
            lines.push(Line::from(Span::styled("✻ Claude", st(ORANGE).add_modifier(Modifier::BOLD))));
            lines.extend(md_lines(text));
        }
        lines.push(Line::default());
    }
    if app.chat_busy {
        const SPIN: [&str; 4] = ["✢", "✳", "✶", "✻"];
        lines.push(Line::from(Span::styled(format!("{} thinking…", SPIN[app.spinner / 2 % 4]), st(ORANGE))));
    }
    let block = panel(" ✻ Claude ");
    let inner = block.inner(area);
    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    // chat_scroll counts up from the newest line, so the view sticks to the bottom by default.
    let total = para.line_count(inner.width) as u16;
    let max = total.saturating_sub(inner.height);
    let scroll = max.saturating_sub(app.chat_scroll.min(max));
    f.render_widget(para.scroll((scroll, 0)).block(block), area);
}

fn draw_tutor(f: &mut Frame, app: &App, area: Rect) {
    match app.tutor_level {
        TutorLevel::Courses => draw_tutor_courses(f, app, area),
        TutorLevel::Materials => draw_tutor_materials(f, app, area),
        TutorLevel::Lesson => draw_tutor_lesson(f, app, area),
        TutorLevel::Notes => draw_tutor_notes(f, app, area),
    }
}

fn draw_tutor_courses(f: &mut Frame, app: &App, area: Rect) {
    let (list, side) = with_preview(area);
    if app.tutor_courses.is_empty() {
        empty(f, list, "No courses yet. Press gs to sync Moodle, then come back here.");
    } else {
        let width = (list.width as usize).saturating_sub(5);
        let rows = app
            .tutor_courses
            .iter()
            .map(|c| {
                let open = app.tasks.iter().filter(|t| !t.done && t.course == c.shortname).count();
                let tag = if open > 0 { format!("{open} open") } else { String::new() };
                Row::Item(Line::from(vec![Span::styled(pad(&c.fullname, width.saturating_sub(9)), st(FG)), Span::styled(format!("{tag:>8}"), st(YELLOW))]))
            })
            .collect();
        draw_buffer(f, list, rows, app.tutor_course, true);
    }
    let Some(side) = side else { return };
    let lines = vec![
        Line::from(Span::styled("✻ Your tutor", st(ORANGE).add_modifier(Modifier::BOLD))),
        Line::default(),
        Line::from(Span::styled("Pick a course, then an exercise or a PDF. Claude reads the pages, teaches you step by step, and saves its own notes on the methods your teacher uses.", st(MUTED))),
        Line::default(),
        Line::from(vec![Span::styled("Enter  ", st(ORANGE)), Span::styled("open a course", st(DIM))]),
        Line::from(vec![Span::styled("s      ", st(ORANGE)), Span::styled("download its files from Moodle", st(DIM))]),
        Line::default(),
        Line::from(Span::styled("Claude only sees what you open here. It has no tools, so it can't read your other files or change anything.", st(DIM))),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel(" Tutor ")), side);
}

fn draw_tutor_materials(f: &mut Frame, app: &App, area: Rect) {
    let (list, side) = with_preview(area);
    let course = app.tutor_courses.get(app.tutor_course).map_or("", |c| c.fullname.as_str());
    let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(list);
    f.render_widget(Paragraph::new(Line::from(vec![Span::raw(" "), Span::styled(course.to_string(), st(BLUE).add_modifier(Modifier::BOLD)), Span::styled(format!("   {} notes", app.tutor_notes.len()), st(DIM))])), head);
    if app.tutor_materials.is_empty() {
        empty(f, body, "No exercises or files yet. Press s to download this course's files.");
    } else {
        let width = (body.width as usize).saturating_sub(4 + 11);
        let mut rows = Vec::new();
        let mut last = "";
        for m in &app.tutor_materials {
            let (section, right, color) = match m {
                Material::Task { due, .. } => ("Open on Moodle", due.map_or(String::new(), fmt_rel), YELLOW),
                Material::File { .. } => ("Files", String::new(), DIM),
            };
            if section != last {
                rows.push(section_header(section));
                last = section;
            }
            let started = tutor::find_session(&app.db, &m.source_key()).ok().flatten().is_some();
            rows.push(Row::Item(Line::from(vec![
                Span::styled(if started { "✻ " } else { "  " }, st(ORANGE)),
                Span::styled(pad(m.title(), width.saturating_sub(2)), st(FG)),
                Span::styled(format!(" {right:>9}"), st(color)),
            ])));
        }
        draw_buffer(f, body, rows, app.tutor_material, true);
    }
    let Some(side) = side else { return };
    let block = panel(" Preview ");
    let inner = block.inner(side);
    f.render_widget(block, side);
    match app.tutor_materials.get(app.tutor_material) {
        Some(Material::Task { title, description, due, .. }) => {
            let mut lines = vec![Line::from(Span::styled(title.clone(), st(FG).add_modifier(Modifier::BOLD)))];
            if let Some(d) = due {
                lines.push(Line::from(Span::styled(format!("due {} ({})", fmt_abs(*d), fmt_rel(*d)), st(due_color(*d, now_ts())))));
            }
            lines.push(Line::default());
            if description.trim().is_empty() {
                lines.push(Line::from(Span::styled("No description. Attachments are downloaded when you start.", st(DIM))));
            } else {
                lines.extend(md_lines(description));
            }
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        Some(Material::File { .. }) => draw_preview_content(f, inner, &app.tutor_preview, None, app),
        None => {}
    }
    let keys = "Enter learn  v view  N restart  n notes  s download  h back";
    if inner.height > 1 {
        f.render_widget(Paragraph::new(Span::styled(fit(keys, inner.width as usize), st(DIM))).style(Style::default().bg(BG)), Rect { y: inner.y + inner.height - 1, height: 1, ..inner });
    }
}

fn draw_tutor_lesson(f: &mut Frame, app: &App, area: Rect) {
    let (chat_area, side) = if area.width >= 100 {
        let [a, b] = Layout::horizontal([Constraint::Min(60), Constraint::Length(34)]).areas(area);
        (a, Some(b))
    } else {
        (area, None)
    };
    let mut lines: Vec<Line> = Vec::new();
    for (role, text) in &app.tutor_chat {
        match role {
            Role::You => {
                lines.push(Line::from(Span::styled("› You", st(BLUE).add_modifier(Modifier::BOLD))));
                lines.extend(text.lines().map(|l| Line::from(Span::styled(l.to_string(), st(FG)))));
                lines.push(Line::default());
            }
            Role::Tutor => {
                lines.push(Line::from(Span::styled("✻ Tutor", st(ORANGE).add_modifier(Modifier::BOLD))));
                lines.extend(md_lines(text));
                lines.push(Line::default());
            }
            Role::Saved => {
                lines.push(Line::from(Span::styled(format!("  ✎ {text}"), st(GREEN))));
                lines.push(Line::default());
            }
        }
    }
    if app.tutor_busy {
        const SPIN: [&str; 4] = ["✢", "✳", "✶", "✻"];
        lines.push(Line::from(Span::styled(format!("{} reading and thinking…", SPIN[app.spinner / 2 % 4]), st(ORANGE))));
    } else if app.tutor_chat.is_empty() {
        lines.push(Line::from(Span::styled("Press i to ask your tutor something.", st(MUTED))));
    }
    let block = panel(format!(" ✻ {} ", fit(&app.tutor_title, (chat_area.width as usize).saturating_sub(8))));
    let inner = block.inner(chat_area);
    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total = para.line_count(inner.width) as u16;
    let max = total.saturating_sub(inner.height);
    let scroll = max.saturating_sub(app.tutor_scroll.min(max));
    f.render_widget(para.scroll((scroll, 0)).block(block), chat_area);

    let Some(side) = side else { return };
    let mut info: Vec<Line> = Vec::new();
    info.push(Line::from(Span::styled("Reading", st(ORANGE).add_modifier(Modifier::BOLD))));
    if app.tutor_docs.is_empty() {
        info.push(Line::from(Span::styled("the task description", st(DIM))));
    }
    for d in &app.tutor_docs {
        let name = d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        info.push(Line::from(Span::styled(fit(&name, side.width.saturating_sub(2) as usize), st(MUTED))));
    }
    info.push(Line::default());
    info.push(Line::from(Span::styled(format!("Claude's notes ({})", app.tutor_notes.len()), st(ORANGE).add_modifier(Modifier::BOLD))));
    if app.tutor_notes.is_empty() {
        info.push(Line::from(Span::styled("None yet for this subject.", st(DIM))));
    }
    for n in app.tutor_notes.iter().take(side.height.saturating_sub(14) as usize) {
        info.push(Line::from(vec![Span::styled("✎ ", st(GREEN)), Span::styled(fit(&n.title, side.width.saturating_sub(4) as usize), st(MUTED))]));
    }
    info.push(Line::default());
    for (k, d) in [("i", "ask"), ("j k", "scroll"), ("v", "view the PDF"), ("n", "read the notes"), ("h", "back")] {
        info.push(Line::from(vec![Span::styled(format!("{k:<5}"), st(ORANGE)), Span::styled(d, st(DIM))]));
    }
    f.render_widget(Paragraph::new(info).block(panel(" Lesson ")), side);
}

fn draw_tutor_notes(f: &mut Frame, app: &App, area: Rect) {
    let (list, side) = with_preview(area);
    if app.tutor_notes.is_empty() {
        empty(f, list, "Claude hasn't saved any notes for this subject yet. They appear here as you learn.");
    } else {
        let width = (list.width as usize).saturating_sub(5);
        let rows = app.tutor_notes.iter().map(|n| Row::Item(Line::from(Span::styled(fit(&n.title, width), st(FG))))).collect();
        draw_buffer(f, list, rows, app.tutor_note, true);
    }
    let Some(side) = side else { return };
    let title = format!(" Claude's notes: {} ", app.tutor_subject);
    let lines = match app.tutor_notes.get(app.tutor_note) {
        Some(n) => {
            let mut l = vec![Line::from(Span::styled(n.title.clone(), st(ORANGE).add_modifier(Modifier::BOLD))), Line::from(Span::styled(format!("updated {}", fmt_rel(n.modified)), st(DIM))), Line::default()];
            l.extend(md_lines(&n.body));
            l.push(Line::default());
            l.push(Line::from(Span::styled("Enter edit  d delete  u undo  h back", st(DIM))));
            l
        }
        None => vec![],
    };
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((app.preview_scroll, 0)).block(panel(title)), side);
}

fn draw_chats(f: &mut Frame, app: &App, area: Rect) {
    match app.chats.level {
        ChatLevel::List => draw_chat_list(f, app, area),
        ChatLevel::Chat => draw_chat(f, app, area),
        ChatLevel::Files => draw_chat_files(f, app, area),
        ChatLevel::Picker => draw_picker(f, app, area),
        ChatLevel::Books => draw_books(f, app, area),
    }
}

fn key_lines<'a>(keys: &[(&'a str, &'a str)]) -> Vec<Line<'a>> {
    keys.iter().map(|(k, d)| Line::from(vec![Span::styled(format!("{k:<7}"), st(ORANGE)), Span::styled(*d, st(DIM))])).collect()
}

fn draw_chat_list(f: &mut Frame, app: &App, area: Rect) {
    let (list, side) = with_preview(area);
    if app.chats.list.is_empty() {
        empty(f, list, "No chats yet. Press o to start one, or B for your books.");
    } else {
        let width = (list.width as usize).saturating_sub(4 + 22);
        let rows = app
            .chats
            .list
            .iter()
            .map(|c| {
                let files = if c.files == 1 { "1 file".to_string() } else { format!("{} files", c.files) };
                Row::Item(Line::from(vec![Span::styled(pad(&c.title, width), st(FG)), Span::styled(format!(" {files:>9}"), st(BLUE)), Span::styled(format!(" {:>9}", fmt_rel(c.updated_at)), st(DIM))]))
            })
            .collect();
        draw_buffer(f, list, rows, app.chats.sel, true);
    }
    let Some(side) = side else { return };
    let mut lines = vec![
        Line::from(Span::styled("✻ Your chats", st(ORANGE).add_modifier(Modifier::BOLD))),
        Line::default(),
        Line::from(Span::styled("Each chat has its own files. Claude reads them, looks up pages in big books, and says which page a number came from.", st(MUTED))),
        Line::default(),
    ];
    lines.extend(key_lines(&[("o", "new chat"), ("Enter", "open"), ("r", "rename"), ("d", "delete"), ("B", "your books")]));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel(" Chats ")), side);
}

fn chat_lines(log: &[(Role, String)], who: &str, busy: bool, spinner: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    for (role, text) in log {
        match role {
            Role::You => {
                lines.push(Line::from(Span::styled("› You", st(BLUE).add_modifier(Modifier::BOLD))));
                lines.extend(text.lines().map(|l| Line::from(Span::styled(l.to_string(), st(FG)))));
                lines.push(Line::default());
            }
            Role::Tutor => {
                lines.push(Line::from(Span::styled(format!("✻ {who}"), st(ORANGE).add_modifier(Modifier::BOLD))));
                lines.extend(md_lines(text));
                lines.push(Line::default());
            }
            Role::Saved => {
                lines.push(Line::from(Span::styled(format!("  ✎ {text}"), st(GREEN))));
                lines.push(Line::default());
            }
        }
    }
    if busy {
        const SPIN: [&str; 4] = ["✢", "✳", "✶", "✻"];
        lines.push(Line::from(Span::styled(format!("{} reading and thinking…", SPIN[spinner / 2 % 4]), st(ORANGE))));
    }
    lines
}

fn draw_chat(f: &mut Frame, app: &App, area: Rect) {
    let (chat_area, side) = if area.width >= 100 {
        let [a, b] = Layout::horizontal([Constraint::Min(60), Constraint::Length(34)]).areas(area);
        (a, Some(b))
    } else {
        (area, None)
    };
    let mut lines = chat_lines(&app.chats.log, "Claude", app.chats.busy, app.spinner);
    if lines.is_empty() {
        let hint = if app.chats.docs.is_empty() { "Press f to add files, or i to ask something." } else { "Press i to ask something about your files." };
        lines.push(Line::from(Span::styled(hint, st(MUTED))));
    }
    let block = panel(format!(" ✻ {} ", fit(&app.chats.title, (chat_area.width as usize).saturating_sub(8))));
    let inner = block.inner(chat_area);
    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max = (para.line_count(inner.width) as u16).saturating_sub(inner.height);
    let scroll = max.saturating_sub(app.chats.scroll.min(max));
    f.render_widget(para.scroll((scroll, 0)).block(block), chat_area);

    let Some(side) = side else { return };
    let mut info = vec![Line::from(Span::styled(format!("Files ({})", app.chats.docs.len()), st(ORANGE).add_modifier(Modifier::BOLD)))];
    if app.chats.docs.is_empty() {
        info.push(Line::from(Span::styled("None yet.", st(DIM))));
    }
    let room = side.height.saturating_sub(10) as usize;
    for d in app.chats.docs.iter().take(room) {
        let name = d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        info.push(Line::from(Span::styled(fit(&name, side.width.saturating_sub(2) as usize), st(MUTED))));
    }
    if app.chats.docs.len() > room {
        info.push(Line::from(Span::styled(format!("+{} more", app.chats.docs.len() - room), st(DIM))));
    }
    info.push(Line::default());
    info.extend(key_lines(&[("i", "ask"), ("f", "files"), ("j k", "scroll"), ("h", "back")]));
    f.render_widget(Paragraph::new(info).block(panel(" This chat ")), side);
}

fn draw_chat_files(f: &mut Frame, app: &App, area: Rect) {
    let (list, side) = with_preview(area);
    if app.chats.docs.is_empty() {
        empty(f, list, "No files in this chat. Press a to add some.");
    } else {
        let width = (list.width as usize).saturating_sub(5);
        let rows = app
            .chats
            .docs
            .iter()
            .map(|d| {
                let name = d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let color = if d.exists() { FG } else { RED };
                Row::Item(Line::from(Span::styled(fit(&name, width), st(color))))
            })
            .collect();
        draw_buffer(f, list, rows, app.chats.file_sel, true);
    }
    let Some(side) = side else { return };
    let mut lines = vec![Line::from(Span::styled(app.chats.title.clone(), st(FG).add_modifier(Modifier::BOLD))), Line::default()];
    if let Some(d) = app.chats.docs.get(app.chats.file_sel) {
        lines.push(Line::from(Span::styled(d.display().to_string(), st(DIM))));
        if !d.exists() {
            lines.push(Line::from(Span::styled("This file has moved or been deleted.", st(RED))));
        }
        lines.push(Line::default());
    }
    lines.extend(key_lines(&[("a", "add files"), ("d", "remove from chat"), ("v", "view"), ("h", "back to the chat")]));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel(" Files in this chat ")), side);
}

fn draw_picker(f: &mut Frame, app: &App, area: Rect) {
    let (list, side) = with_preview(area);
    let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(list);
    match &app.chats.browser {
        None => {
            f.render_widget(Paragraph::new(Span::styled(" Add files from", st(BLUE).add_modifier(Modifier::BOLD))), head);
            let rows = app.picker_roots().iter().map(|(name, path)| {
                let count = if path.is_dir() { String::new() } else { "  empty".to_string() };
                Row::Item(Line::from(vec![Span::styled(name.to_string(), st(FG)), Span::styled(count, st(DIM))]))
            }).collect();
            draw_buffer(f, body, rows, app.chats.root_sel, true);
        }
        Some(browser) => {
            f.render_widget(Paragraph::new(Span::styled(format!(" {}", fit(&browser.location(), head.width as usize)), st(BLUE).add_modifier(Modifier::BOLD))), head);
            let entries = app.picker_entries();
            if entries.is_empty() {
                empty(f, body, "This folder is empty.");
            } else {
                let width = (body.width as usize).saturating_sub(4 + 6);
                let rows = entries
                    .iter()
                    .map(|e| {
                        let added = app.chats.docs.contains(&e.path);
                        let (icon, color) = file_icon(e, app.cfg.nerd_icons);
                        Row::Item(Line::from(vec![
                            Span::styled(if added { "✓ " } else { "  " }, st(GREEN)),
                            Span::styled(format!("{icon} "), st(color)),
                            Span::styled(fit(&e.name, width), st(if e.is_dir { BLUE } else if added { GREEN } else { FG })),
                        ]))
                    })
                    .collect();
                draw_buffer(f, body, rows, app.chats.entry_sel, true);
            }
        }
    }
    let Some(side) = side else { return };
    let block = panel(format!(" {} added ", app.chats.docs.len()));
    let inner = block.inner(side);
    f.render_widget(block, side);
    let keys = "Enter open or add  x add  / filter  h back";
    if app.chats.browser.is_some() && inner.height > 2 {
        let [view, foot] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
        draw_preview_content(f, view, &app.chats.preview, None, app);
        f.render_widget(Paragraph::new(Span::styled(fit(keys, foot.width as usize), st(DIM))), foot);
    } else {
        let note = "Pick where to look. Files stay where they are, and Claude only sees the ones you add.";
        f.render_widget(Paragraph::new(Span::styled(note, st(MUTED))).wrap(Wrap { trim: false }), inner);
    }
}

fn draw_books(f: &mut Frame, app: &App, area: Rect) {
    let (list, side) = with_preview(area);
    let rows_data = app.book_rows();
    if rows_data.is_empty() {
        empty(f, list, "No books yet. Press L to log in to digi4school and see yours.");
    } else {
        let width = (list.width as usize).saturating_sub(5 + 2);
        let mut rows = Vec::new();
        let mut last = "";
        for row in &rows_data {
            let section = match row {
                BookRow::Local(_) => "On this computer",
                BookRow::Remote(_) => "On your digi4school account",
            };
            if section != last {
                rows.push(section_header(section));
                last = section;
            }
            let (title, color) = match row {
                BookRow::Local(p) => (p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), FG),
                BookRow::Remote(b) => (format!("{} ({})", b.title, b.year), MUTED),
            };
            rows.push(Row::Item(Line::from(Span::styled(fit(&title, width), st(color)))));
        }
        draw_buffer(f, list, rows, app.chats.book_sel, true);
    }
    let Some(side) = side else { return };
    let mut lines = vec![Line::from(Span::styled("Your digi4school books", st(ORANGE).add_modifier(Modifier::BOLD))), Line::default()];
    if let Some(job) = &app.chats.job {
        lines.push(Line::from(Span::styled(fit(&job.title, side.width.saturating_sub(2) as usize), st(FG))));
        lines.push(Line::from(Span::styled(job.status.clone(), st(BLUE))));
        lines.push(Line::default());
    }
    let account = if app.cfg.digi4school_email.is_empty() { "not set".to_string() } else { app.cfg.digi4school_email.clone() };
    lines.push(Line::from(vec![Span::styled("account  ", st(DIM)), Span::styled(account, st(MUTED))]));
    let state = if app.chats.password.is_some() { "logged in until you quit" } else { "asked each time you open LockIn" };
    lines.push(Line::from(vec![Span::styled("password ", st(DIM)), Span::styled(state, st(MUTED))]));
    lines.push(Line::default());
    lines.extend(key_lines(&[("L", "log in, list your books"), ("Enter", "download or read"), ("x", "stop a download"), ("d", "delete from this computer"), ("h", "back")]));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("These books are licensed to your account. They stay on this computer, so please don't share them.", st(DIM))));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel(" Books ")), side);
}

// ---- popups ---------------------------------------------------------------

const LEADER_KEYS: &[(&str, &str)] = &[
    ("t", "today"),
    ("T", "tasks"),
    ("c", "calendar"),
    ("n", "notes"),
    ("f", "find note"),
    ("d", "daily note"),
    ("h", "habits"),
    ("g", "grades"),
    ("p", "focus timer"),
    ("a", "ask claude"),
    ("l", "learn with your tutor"),
    ("C", "chats with your files"),
    ("m", "open moodle cli"),
    (",", "settings"),
    ("s", "sync moodle"),
    ("b", "claude briefing"),
    ("?", "help"),
    ("q", "quit"),
];

const G_KEYS: &[(&str, &str)] = &[
    ("g", "go to top"),
    ("t", "next tab (3gt = tab 3)"),
    ("T", "previous tab"),
    ("s", "sync moodle"),
    ("x", "open link in browser"),
    ("f", "focus on this task"),
];

fn draw_which_key(f: &mut Frame, body: Rect, title: &str, keys: &[(&str, &str)]) {
    let height = (keys.len() as u16 + 2).min(body.height);
    let width = 34.min(body.width);
    let rect = Rect { x: body.x + body.width - width, y: body.y + body.height - height, width, height };
    let lines: Vec<Line> = keys.iter().map(|(k, d)| Line::from(vec![Span::styled(format!(" {k}  "), st(ORANGE).add_modifier(Modifier::BOLD)), Span::styled(*d, st(FG))])).collect();
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(PANEL)).block(panel(format!(" {title} "))), rect);
}

const HELP: &str = "\
# Move around
j k  move   gg G  top, bottom   5j  repeat   Ctrl-d Ctrl-u  half page
J K  scroll the details pane    Tab, gt, gT  change tab   3gt  go to tab 3
/  fuzzy search (Esc clears)    Space  shortcut menu

# Tasks (Today and Tasks tabs)
o  new task   e  edit title   D  set due date   x  done, or reopen
+ -  priority   dd  delete    u  undo           f  filter all, Moodle, mine
zd  show finished   gx  open in browser   gf  focus on this task   L  learn it
Enter on a Moodle task opens it in the Moodle CLI, where u uploads files.
Enter on your own task edits its notes.

Quick add:  Read chapter 4 @fri 18:00 !! #AM ~45m
Dates:  today, tom, fri, +3d, +4h, 14.11., 2026-12-01 08:15, 21:00

# Calendar
h j k l  move by day or week   H L  change month   t  today
o  add event   a  add task   Enter or e  edit   d  delete (asks first)
J K or n N  pick an item on the day   + -  move it a day   u  undo
x  tick a task. Enter on a Moodle task opens it in the Moodle CLI.

In the form: Tab next field, Enter save, Esc cancel, Ctrl-t event or task.
Dates: fri, tom, +3d, 14.11.   Times: 14:00, 9   Lengths: 45m, 1h, 1.5h

# Notes (your Obsidian vault, with Yazi's keys)
h l  parent, open   Enter  open   a  create (end with / for a folder)
r  rename   d  move to .trash   u  undo   y  copy   x  cut   p  paste
/  filter this folder   z or Space f  find a note anywhere in the vault
s  sort   .  hidden files   ~  vault root   D  daily note   J K  scroll preview

# PDFs (Enter on a PDF, :pdf file, or lockin pdf file)
j k  scroll   J K or Space  page   12G  go to page 12   gg G  first, last
+ -  zoom   0  reset   w  fit width   h l  pan   t  text view
/  find text   n N  next, previous   o  default app   q  back

# Habits
x  tick the selected day   h l  pick a day   o  new   dd  delete

# Grades
J K  scroll items   zp  show past courses

# Focus
s  start   p  pause   b  break   x  stop   + -  change by 5 min
gf on a task starts a session tied to that task.

# Claude
i  ask   j k  scroll   c  clear the chat

# Tutor
Enter or l  open   h or Esc  back   s  download the course's files
On an exercise or file: Enter learn   v view   N start over   n Claude's notes
In a lesson: i ask   j k scroll   v view the PDF   n read the notes
In Claude's notes: Enter edit   d delete   u undo
L on a Moodle task, or on a file in Notes, starts a lesson on it.
Only what you open goes to Claude.

# Chats
o  new chat   Enter open   r rename   d delete   B your books
In a chat: i ask   f files   j k scroll   h back
Files: a add   d remove   v view     Adding: Enter open or add   x add   h back
Books: L log in and list   Enter download or read   x stop   d delete
Claude finds pages in big books itself, and says where each number came from.

# Settings
j k  move   Enter or x  change   h l  step a value

# Commands
:sync   :brief   :add ...   :event ...   :note name   :daily   :habit name
:focus 25   :stop   :filter moodle   :notify   :moodle   :pdf file   :chats   :books   :settings   :q
";

fn draw_help(f: &mut Frame, app: &App, body: Rect) {
    let width = 78.min(body.width);
    let height = body.height.saturating_sub(2).min(40);
    let rect = Rect { x: body.x + (body.width - width) / 2, y: body.y + (body.height - height) / 2, width, height };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(md_lines(HELP))
            .wrap(Wrap { trim: false })
            .scroll((app.help_scroll, 0))
            .style(Style::default().bg(PANEL))
            .block(panel(" Help: j k scroll, any other key closes ")),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_markdown_is_parsed() {
        let text = |spans: Vec<Span>| spans.iter().map(|s| s.content.to_string()).collect::<Vec<_>>();
        let base = st(FG);
        assert_eq!(text(inline_spans("a **b** `c` [[Page|d]] e", base)), ["a ", "b", " ", "c", " ", "d", " e"]);
        assert_eq!(text(inline_spans("[[Plain Page]]", base)), ["Plain Page"]);
        assert_eq!(text(inline_spans("no **close", base)), ["no **close"]);
        assert_eq!(text(inline_spans("", base)), Vec::<String>::new());
        assert_eq!(md_lines("- [x] done\n---\n## Hi\n#tag").len(), 4);
    }

    #[test]
    fn course_tags_are_readable() {
        assert_eq!(course_tag("POSE_OO_2IHIF_2627"), "POSE");
        assert_eq!(course_tag("WMC2IHIFHASLM2627"), "WMC");
        assert_eq!(course_tag("AM2IHIF2026/27"), "AM");
        assert_eq!(course_tag("STSW 1IHIF Klewein 2526"), "STSW");
        assert_eq!(course_tag(""), "");
    }

    #[test]
    fn fit_respects_display_width() {
        assert_eq!(fit("Übung", 10), "Übung");
        assert_eq!(fit("abcdefgh", 5), "abcd…");
        assert_eq!(fit("abc", 0), "");
        assert_eq!(pad("ab", 4), "ab  ");
    }

    /// Drives the whole app with real key presses against a throwaway database and vault.
    #[test]
    fn every_view_renders_and_keys_work_at_any_size() {
        use ratatui::backend::TestBackend;
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::Terminal;

        let root = std::env::temp_dir().join(format!("lockin-test-{}", std::process::id()));
        let vault = root.join("vault");
        std::fs::create_dir_all(vault.join("School")).unwrap();
        std::fs::write(vault.join("School/Mathe Übung.md"), "# Mathe\n- **wichtig**\n").unwrap();
        // SAFETY: this is the only test that touches the environment or opens the real paths.
        unsafe { std::env::set_var("LOCKIN_HOME", &root) };
        std::fs::create_dir_all(&root).unwrap();

        let cfg = crate::config::Config { vault: vault.display().to_string(), ..Default::default() };
        let mut app = App::new(cfg).unwrap();
        let press = |app: &mut App, keys: &str| {
            for c in keys.chars() {
                let code = match c {
                    '\n' => KeyCode::Enter,
                    '\x1b' => KeyCode::Esc,
                    '\t' => KeyCode::Tab,
                    c => KeyCode::Char(c),
                };
                app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
            }
        };
        let screen = |app: &mut App, w: u16, h: u16| -> String {
            app.prepare();
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| draw(f, app)).unwrap();
            let buf = t.backend().buffer().clone();
            buf.content().chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>() + "\n").collect()
        };

        // Add through the modal prompt, then edit, prioritize and tick it off.
        press(&mut app, "ggoEssay schreiben @fri 18:00 !! #STSW ~45m\n");
        assert_eq!(app.tasks.len(), 1, "{}", app.msg);
        assert_eq!(app.tasks[0].priority, 2);
        assert_eq!(app.tasks[0].effort_min, Some(45));
        assert!(screen(&mut app, 120, 30).contains("Essay schreiben"));
        press(&mut app, ":tasks\n");
        press(&mut app, "x");
        assert!(app.tasks[0].done);
        press(&mut app, "u");
        assert!(!app.tasks[0].done, "u undoes the checkmark");
        press(&mut app, "dd");
        assert!(app.tasks.is_empty());
        press(&mut app, "u");
        assert_eq!(app.tasks.len(), 1, "u restores a deleted task");

        // A bad date reports an error instead of saving nonsense.
        press(&mut app, "oThing @notaday\n");
        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.msg_kind, MsgKind::Error);

        // Add an event on the selected day through the form.
        press(&mut app, ":cal\nloExam\t\t14:00\t2h\tR12\n");
        assert_eq!(app.events.len(), 1, "{}", app.msg);
        assert_eq!(app.mode, Mode::Normal);
        let tomorrow = today() + Duration::days(1);
        assert_eq!(date_of(app.events[0].start), tomorrow);
        assert_eq!(app.events[0].end - app.events[0].start, 7200);
        assert_eq!(app.events[0].location, "R12");
        assert!(screen(&mut app, 120, 34).contains("14:00–16:00"));

        // Edit it: Enter opens the form with its values, and typing replaces the title.
        press(&mut app, "\n");
        assert_eq!(app.mode, Mode::Form);
        assert!(screen(&mut app, 120, 34).contains("Edit event"));
        press(&mut app, "Final Exam\n");
        assert_eq!(app.events[0].title, "Final Exam");
        assert_eq!(app.events[0].location, "R12", "untouched fields stay");
        press(&mut app, "u");
        assert_eq!(app.events[0].title, "Exam", "u undoes an edit");

        // A bad value keeps the form open with what you typed.
        press(&mut app, "\n\t\t99:99\n");
        assert_eq!(app.mode, Mode::Form);
        assert!(app.form.as_ref().unwrap().error.is_some());
        assert_eq!(app.events[0].start, crate::timeutil::ts_of(tomorrow, chrono::NaiveTime::from_hms_opt(14, 0, 0).unwrap()));
        press(&mut app, "\x1b");
        assert_eq!(app.mode, Mode::Normal);

        // Move it a day and the calendar follows it, then undo.
        press(&mut app, "+");
        assert_eq!(date_of(app.events[0].start), tomorrow + Duration::days(1));
        assert_eq!(app.cal_date, tomorrow + Duration::days(1));
        assert_eq!(app.events[0].end - app.events[0].start, 7200, "keeps its length");
        press(&mut app, "u");
        assert_eq!(date_of(app.events[0].start), tomorrow);
        app.cal_date = tomorrow;
        app.sel_agenda = 0;

        // Delete asks first. Any other key cancels, and d or y confirms.
        press(&mut app, "dn");
        assert_eq!(app.events.len(), 1);
        press(&mut app, "dd");
        assert!(app.events.is_empty(), "dd deletes");
        press(&mut app, "u");
        assert_eq!(app.events.len(), 1, "u restores it");
        assert_eq!(app.events[0].location, "R12");

        // On an empty day, Enter starts a new event. Ctrl-t makes it a task instead.
        let before = app.tasks.len();
        press(&mut app, "l\n");
        assert_eq!(app.mode, Mode::Form);
        app.on_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
        press(&mut app, "Lernen\n");
        assert_eq!(app.tasks.len(), before + 1, "{}", app.msg);
        assert_eq!(date_of(app.tasks.iter().find(|t| t.title == "Lernen").unwrap().due.unwrap()), tomorrow + Duration::days(1));
        // Tasks can be edited from the calendar too.
        press(&mut app, "\nStudy\n");
        assert!(app.tasks.iter().any(|t| t.title == "Study"));
        press(&mut app, "dy");
        assert!(!app.tasks.iter().any(|t| t.title == "Study"));
        press(&mut app, ":cal\n");
        app.cal_date = tomorrow;

        press(&mut app, ":habit Lesen\n:habits\nx");
        assert_eq!(app.habits[0].streak(), 1);

        // Browse into a folder, filter it, and come back with the folder still selected.
        press(&mut app, ":notes\n");
        assert_eq!(app.visible_entries()[0].name, "School");
        press(&mut app, "l");
        assert!(app.browser.cwd.ends_with("School"));
        press(&mut app, "/mth");
        assert_eq!(app.visible_entries().len(), 1);
        press(&mut app, "x");
        assert!(app.visible_entries().is_empty());
        press(&mut app, "\x1b");
        assert_eq!(app.visible_entries().len(), 1);
        press(&mut app, "h");
        assert_eq!(app.browser.cwd, vault);
        press(&mut app, "h");
        assert_eq!(app.browser.cwd, vault, "h stops at the vault root");

        // Create, rename and trash a note, undoing each change.
        press(&mut app, "aNeu\n");
        assert!(vault.join("Neu.md").exists(), "{}", app.msg);
        assert!(matches!(app.want_editor.take(), Some(EditTarget::Note(_))));
        press(&mut app, "rx\n");
        assert!(vault.join("Neu.mdx").exists(), "{}", app.msg);
        press(&mut app, "u");
        assert!(vault.join("Neu.md").exists(), "u undoes a rename");
        press(&mut app, "dn");
        assert!(vault.join("Neu.md").exists(), "n cancels the delete");
        press(&mut app, "dy");
        assert!(!vault.join("Neu.md").exists());
        assert!(vault.join(".trash/Neu.md").exists(), "delete goes to the vault's .trash");
        press(&mut app, "u");
        assert!(vault.join("Neu.md").exists(), "u restores from .trash");

        // Find across the whole vault, which jumps to the note's folder and opens it.
        press(&mut app, "zmathe");
        assert_eq!(app.find_matches().len(), 1);
        assert!(screen(&mut app, 120, 30).contains("Mathe Übung"));
        press(&mut app, "\n");
        assert!(app.browser.cwd.ends_with("School"));
        assert!(matches!(app.want_editor.take(), Some(EditTarget::Note(p)) if p.ends_with("Mathe Übung.md")));
        let notes_screen = screen(&mut app, 120, 30);
        assert!(notes_screen.contains("vault/School") && notes_screen.contains("Mathe"), "{notes_screen}");

        // Settings change on the spot, reject bad input and persist to disk.
        press(&mut app, ":settings\n\n");
        assert!(!app.cfg.daily_enabled, "{}", app.msg);
        press(&mut app, "j\n, 7:00\n");
        assert_eq!(app.cfg.daily_times, vec!["07:00", "15:00"], "{}", app.msg);
        press(&mut app, "\nnonsense\n");
        assert_eq!(app.msg_kind, MsgKind::Error);
        assert_eq!(app.cfg.daily_times, vec!["07:00", "15:00"]);
        assert!(!crate::config::load().unwrap().daily_enabled, "saved to the config file");
        assert!(screen(&mut app, 120, 40).contains("07:00, 15:00"));

        // Enter opens a Moodle assignment in the CLI and anything else elsewhere.
        let task = |ext: &str, url: &str| crate::db::RemoteTask {
            ext_id: ext.into(),
            title: ext.into(),
            course: "AM".into(),
            due: now_ts() + 3600,
            url: url.into(),
            action: String::new(),
            notes: String::new(),
        };
        let snapshot = crate::db::Snapshot {
            window_from: 0,
            tasks: vec![task("hw", "https://x/mod/assign/view.php?id=42&action=editsubmission")],
            events: vec![],
            grades: vec![],
            inbox: vec![],
        };
        app.db.apply_snapshot(&snapshot).unwrap();
        app.refresh();
        press(&mut app, ":filter moodle\n\n");
        assert!(matches!(app.want_editor.take(), Some(EditTarget::Assignment(42, c)) if c == "AM"));
        press(&mut app, ":filter all\n:moodle\n");
        assert!(matches!(app.want_editor.take(), Some(EditTarget::MoodleApp)));

        // Every view at wide, narrow and tiny sizes, with popups and the timer.
        press(&mut app, ":focus 25\n");
        assert!(app.focus.is_some());
        for (view, _) in VIEWS {
            app.view = view;
            for (w, h) in [(160, 45), (120, 30), (80, 24), (60, 16), (30, 8), (10, 4)] {
                screen(&mut app, w, h);
                app.help_open = true;
                screen(&mut app, w, h);
                app.help_open = false;
                app.pending = Some(' ');
                screen(&mut app, w, h);
                app.pending = None;
                if view == View::Calendar {
                    app.form = Some(crate::form::Form::new_event(today()));
                    app.mode = Mode::Form;
                    screen(&mut app, w, h);
                    app.form = None;
                    app.mode = Mode::Normal;
                }
                if view == View::Notes {
                    app.mode = Mode::Input(Prompt::VaultFind);
                    screen(&mut app, w, h);
                    app.mode = Mode::Normal;
                }
            }
        }

        // Tutor: browse a course's materials, open a lesson, read Claude's notes.
        let moodle_dir = root.join("moodle/AM");
        std::fs::create_dir_all(&moodle_dir).unwrap();
        std::fs::write(moodle_dir.join("Blatt.txt"), "Vereinfachen Sie").unwrap();
        let courses = serde_json::json!([
            {"shortname": "AM", "fullname": "Mathematik", "current": true, "folder": moodle_dir},
            {"shortname": "OLD", "fullname": "Old course", "current": false, "folder": "/nonexistent"},
        ]);
        app.db.set_meta("courses", &courses.to_string()).unwrap();
        tutor::save_note("am", "Potenzgesetze", "(a^m)^n = a^(m·n)").unwrap();
        press(&mut app, ":tutor\n");
        assert_eq!(app.tutor_courses.len(), 1, "past courses are hidden");
        assert!(screen(&mut app, 120, 30).contains("Mathematik"));
        press(&mut app, "\n");
        assert_eq!(app.tutor_level, TutorLevel::Materials);
        assert_eq!(app.tutor_materials.len(), 2, "the open Moodle task, then the file");
        assert!(matches!(app.tutor_materials[0], Material::Task { .. }));
        assert!(screen(&mut app, 120, 30).contains("Blatt.txt"));
        press(&mut app, "\n");
        assert_eq!(app.tutor_level, TutorLevel::Lesson);
        assert!(app.tutor_busy, "a new lesson starts by itself");
        assert!(screen(&mut app, 120, 30).contains("Potenzgesetze"), "Claude's notes show in the lesson");
        app.tutor_busy = false;
        press(&mut app, "iWas ist ein Bruch?\n");
        assert_eq!(app.tutor_chat.last().unwrap().1, "Was ist ein Bruch?");
        assert!(app.tutor_busy);
        press(&mut app, "ialso this\n");
        assert_eq!(app.msg_kind, MsgKind::Error, "one question at a time");
        app.tutor_busy = false;
        press(&mut app, "n");
        assert_eq!(app.tutor_level, TutorLevel::Notes);
        assert!(screen(&mut app, 120, 30).contains("a^(m·n)"));
        press(&mut app, "dn");
        assert_eq!(app.tutor_notes.len(), 1, "n cancels");
        press(&mut app, "dy");
        assert!(app.tutor_notes.is_empty());
        press(&mut app, "u");
        assert_eq!(app.tutor_notes.len(), 1, "u restores a deleted note");
        press(&mut app, "\n");
        assert!(matches!(app.want_editor.take(), Some(EditTarget::Note(_))));
        for (level, session) in [(TutorLevel::Courses, None), (TutorLevel::Materials, None), (TutorLevel::Lesson, app.tutor_session), (TutorLevel::Notes, app.tutor_session)] {
            app.tutor_level = level;
            app.tutor_session = session;
            for (w, h) in [(160, 45), (120, 30), (80, 24), (60, 16), (30, 8), (10, 4)] {
                screen(&mut app, w, h);
            }
        }
        // Reopening keeps the conversation, and start-over asks before erasing it.
        app.tutor_level = TutorLevel::Materials;
        app.tutor_session = None;
        app.tutor_material = 0;
        press(&mut app, "\n");
        assert_eq!(app.tutor_chat.len(), 1, "reopening continues the lesson");
        assert!(!app.tutor_busy, "and doesn't start a new request");
        press(&mut app, "h");
        press(&mut app, "Nn");
        assert_eq!(app.mode, Mode::Normal);
        assert!(tutor::find_session(&app.db, &app.tutor_materials[0].source_key()).unwrap().is_some());
        press(&mut app, "Ny");
        assert!(app.tutor_chat.is_empty(), "start-over erases only after you confirm");
        app.tutor_busy = false;
        press(&mut app, "hh");
        assert_eq!(app.tutor_level, TutorLevel::Courses);

        // Chats: make one, add files from the picker, ask, remove a file, delete the chat.
        press(&mut app, ":chats\n");
        assert_eq!(app.view, View::Chats);
        assert!(screen(&mut app, 120, 30).contains("No chats yet"));
        press(&mut app, "oPhysik\n");
        assert_eq!(app.chats.level, ChatLevel::Files, "a new chat goes straight to adding files");
        assert_eq!(app.chats.title, "Physik");
        press(&mut app, "a");
        assert_eq!(app.chats.level, ChatLevel::Picker);
        assert!(screen(&mut app, 120, 30).contains("Moodle downloads"));
        app.chats.root_sel = 2;
        press(&mut app, "\n");
        assert!(app.chats.browser.is_some());
        let school = app.picker_entries().iter().position(|e| e.name == "School").unwrap();
        app.chats.entry_sel = school;
        press(&mut app, "l");
        assert_eq!(app.picker_entries().len(), 1);
        press(&mut app, "\n");
        assert_eq!(app.chats.docs.len(), 1, "{}", app.msg);
        assert!(screen(&mut app, 120, 30).contains("Mathe Übung"));
        press(&mut app, "x");
        assert!(app.chats.docs.is_empty(), "x takes it back out");
        press(&mut app, "x");
        press(&mut app, "hhh");
        assert_eq!(app.chats.level, ChatLevel::Files);
        assert_eq!(app.chats.docs.len(), 1);
        press(&mut app, "h");
        assert_eq!(app.chats.level, ChatLevel::Chat);
        press(&mut app, "iWie hoch ist die Dichte?\n");
        assert!(app.chats.busy);
        assert_eq!(app.chats.log[0].1, "Wie hoch ist die Dichte?");
        press(&mut app, "ianother\n");
        assert_eq!(app.msg_kind, MsgKind::Error, "one question at a time");
        assert!(screen(&mut app, 120, 30).contains("Mathe Übung"), "the chat lists its files");
        app.chats.busy = false;
        press(&mut app, "f");
        press(&mut app, "d");
        assert!(app.chats.docs.is_empty());
        assert!(vault.join("School/Mathe Übung.md").exists(), "removing from a chat leaves the file alone");
        press(&mut app, "hh");
        assert_eq!(app.chats.level, ChatLevel::List);
        press(&mut app, "rx\n");
        assert_eq!(app.chats.list[0].title, "Physikx");
        // The password is masked on screen, kept in memory only, and never reaches the config.
        press(&mut app, ":books\n");
        assert_eq!(app.chats.level, ChatLevel::Books);
        press(&mut app, "Lnot-an-email\n");
        assert_eq!(app.msg_kind, MsgKind::Error);
        press(&mut app, "Lme@example.com\n");
        assert_eq!(app.mode, Mode::Input(Prompt::BooksPassword));
        press(&mut app, "hunter2secret");
        let masked = screen(&mut app, 120, 30);
        assert!(!masked.contains("hunter2") && masked.contains("•••••"), "{masked}");
        press(&mut app, "\n");
        assert_eq!(app.chats.password.as_deref(), Some("hunter2secret"));
        assert!(app.input.is_empty(), "the password doesn't stay in the input buffer");
        let on_disk = std::fs::read_to_string(crate::config::config_path()).unwrap();
        assert!(on_disk.contains("me@example.com") && !on_disk.contains("hunter2"), "{on_disk}");
        // A rejected login forgets the password, so the next try asks again.
        app.on_book_event(serde_json::json!({"event": "error", "message": "Login failed: bad login"}));
        assert!(app.chats.password.is_none());
        assert!(!app.chats.listing);
        app.on_book_event(serde_json::json!({"event": "books", "books": [{"title": "Physik 2", "year": 2025}]}));
        assert!(screen(&mut app, 120, 30).contains("Physik 2 (2025)"));
        app.chats.password = Some("pw".into());
        app.chats.book_sel = 0;
        press(&mut app, "\n");
        assert_eq!(app.chats.job.as_ref().unwrap().title, "Physik 2");
        app.on_book_event(serde_json::json!({"event": "progress", "volume": "v", "page": 3, "total": 10}));
        assert!(screen(&mut app, 120, 30).contains("page 3/10"));
        app.on_book_event(serde_json::json!({"event": "done", "title": "Physik 2", "count": 1}));
        assert!(app.chats.job.is_none());
        for level in [ChatLevel::List, ChatLevel::Chat, ChatLevel::Files, ChatLevel::Picker, ChatLevel::Books] {
            app.chats.level = level;
            for (w, h) in [(160, 45), (120, 30), (80, 24), (60, 16), (30, 8), (10, 4)] {
                screen(&mut app, w, h);
            }
        }
        app.chats.level = ChatLevel::List;
        press(&mut app, "dn");
        assert_eq!(app.chats.list.len(), 1, "n cancels");
        press(&mut app, "dy");
        assert!(app.chats.list.is_empty());

        // Quitting during a focus session asks first.
        press(&mut app, ":q\n");
        assert!(!app.quit);
        press(&mut app, "n");
        assert!(!app.quit);
        press(&mut app, ":stop\n:q\n");
        assert!(app.quit);

        std::fs::remove_dir_all(&root).ok();
    }

    /// Prints each view as text for checking layout. Run it with LOCKIN_HOME set to a copy of
    /// your data: `LOCKIN_HOME=/some/copy cargo test dump_screens -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_screens() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let cfg = crate::config::load().unwrap();
        let mut app = App::new(cfg).unwrap();
        let (w, h) = (130u16, 34u16);
        for (view, name) in VIEWS {
            app.view = view;
            app.prepare();
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| draw(f, &app)).unwrap();
            let buf = t.backend().buffer().clone();
            println!("===== {name}");
            for row in buf.content().chunks(w as usize) {
                println!("{}", row.iter().map(|c| c.symbol()).collect::<String>().trim_end());
            }
        }
    }

    #[test]
    fn big_time_has_five_equal_rows() {
        let rows = big_time(25 * 60);
        assert_eq!(rows.len(), 5);
        assert!(rows.iter().all(|r| r.chars().count() == rows[0].chars().count()));
    }
}
