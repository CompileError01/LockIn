use crate::config::{config_path, db_path, find_bin, Config};
use crate::db::{Briefing, Db, Event, Grade, Habit, InboxItem, Task};
use crate::files::{self, Browser, Entry, Preview, Sort, Yank};
use crate::form::{self, Form, Target};
use crate::services;
use crate::settings::{self, Kind};
use crate::timeutil::{date_of, now_ts, parse_quick, parse_when, today, ts_of};
use anyhow::{bail, Result};
use chrono::{Datelike, Duration, Local, Months, NaiveDate, NaiveTime};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Today,
    Tasks,
    Calendar,
    Notes,
    Habits,
    Grades,
    Focus,
    Claude,
    Settings,
}

pub const VIEWS: [(View, &str); 9] = [
    (View::Today, "Today"),
    (View::Tasks, "Tasks"),
    (View::Calendar, "Calendar"),
    (View::Notes, "Notes"),
    (View::Habits, "Habits"),
    (View::Grades, "Grades"),
    (View::Focus, "Focus"),
    (View::Claude, "Claude"),
    (View::Settings, "Settings"),
];

#[derive(Clone, PartialEq, Debug)]
pub enum Prompt {
    Command,
    Search,
    NewTask(Option<NaiveDate>),
    EditTitle(i64),
    SetDue(i64),
    NewNote,
    Rename(PathBuf),
    VaultFind,
    NewHabit,
    Chat,
    Setting(settings::Key),
}

#[derive(Clone, PartialEq, Debug)]
pub enum Confirm {
    TrashNote(PathBuf),
    DeleteHabit(i64, String),
    DeleteAgenda(String),
    Quit,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Mode {
    Normal,
    Input(Prompt),
    Confirm(Confirm),
    Form,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TaskFilter {
    All,
    Moodle,
    Mine,
}

#[derive(Clone, PartialEq, Debug)]
pub enum MsgKind {
    Info,
    Error,
}

enum Undo {
    Task(Task),
    RemoveTask(i64),
    Event(Event),
    RemoveEvent(i64),
    Move { from: PathBuf, to: PathBuf },
}

/// Things that take over the terminal for a while.
pub enum EditTarget {
    Note(PathBuf),
    TaskNotes(i64, PathBuf),
    Config,
    Assignment(i64, String),
    MoodleApp,
    Pdf(PathBuf),
}

enum Bg {
    Sync(Result<String, String>),
    Brief(Result<String, String>),
    Chat(Result<String, String>),
}

pub struct NoteEntry {
    pub path: PathBuf,
    pub rel: String,
    pub modified: i64,
}

#[derive(Clone)]
pub enum AgendaItem {
    Task(Task),
    Event(Event),
}

impl AgendaItem {
    pub fn time(&self) -> i64 {
        match self {
            AgendaItem::Task(t) => t.due.unwrap_or(0),
            AgendaItem::Event(e) => e.start,
        }
    }
    pub fn title(&self) -> &str {
        match self {
            AgendaItem::Task(t) => &t.title,
            AgendaItem::Event(e) => &e.title,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Work,
    Break,
}

pub struct Focus {
    pub phase: Phase,
    pub task_id: Option<i64>,
    pub title: String,
    pub minutes: i64,
    started: Instant,
    banked: u64,
    pub paused: bool,
}

impl Focus {
    fn elapsed(&self) -> u64 {
        self.banked + if self.paused { 0 } else { self.started.elapsed().as_secs() }
    }
    pub fn remaining(&self) -> u64 {
        (self.minutes.max(0) as u64 * 60).saturating_sub(self.elapsed())
    }
    pub fn total_secs(&self) -> u64 {
        self.minutes.max(0) as u64 * 60
    }
}

pub struct App {
    pub cfg: Config,
    pub db: Db,
    pub view: View,
    pub mode: Mode,
    pub input: Vec<char>,
    pub cursor: usize,
    pub pending: Option<char>,
    pub count: Option<usize>,
    pub msg: String,
    pub msg_kind: MsgKind,
    pub quit: bool,
    pub want_editor: Option<EditTarget>,
    pub help_open: bool,
    pub help_scroll: u16,

    pub tasks: Vec<Task>,
    pub events: Vec<Event>,
    pub habits: Vec<Habit>,
    pub grades: Vec<Grade>,
    pub inbox: Vec<InboxItem>,
    pub briefing: Option<Briefing>,
    pub notes: Vec<NoteEntry>,
    pub focus_today: i64,
    pub last_sync: Option<i64>,

    pub sel_today: usize,
    pub sel_tasks: usize,
    pub sel_notes: usize,
    pub sel_habits: usize,
    pub sel_grades: usize,
    pub sel_agenda: usize,
    pub habit_col: i64,
    pub cal_date: NaiveDate,
    pub filter: TaskFilter,
    pub show_done: bool,
    pub show_past: bool,
    pub search: String,
    pub preview_scroll: u16,
    pub browser: Browser,
    pub preview: Preview,
    pub find_sel: usize,
    preview_path: Option<PathBuf>,
    pub form: Option<Form>,
    pub sel_settings: usize,
    pub timer_next: String,
    last_sync_try: Instant,

    pub focus: Option<Focus>,
    pub chat: Vec<(bool, String)>,
    pub chat_scroll: u16,
    pub chat_busy: bool,
    pub syncing: bool,
    pub briefing_busy: bool,
    pub spinner: usize,

    undo: Vec<Undo>,
    history: Vec<String>,
    history_pos: Option<usize>,
    last_version: i64,
    last_reload: Instant,
    tx: Sender<Bg>,
    rx: Receiver<Bg>,
}

const COMMANDS: &[&str] = &[
    "add", "brief", "calendar", "claude", "config", "daily", "event", "filter", "focus", "grades", "habit", "habits", "help", "moodle", "note",
    "notes", "notify", "pdf", "q", "qa", "settings", "stop", "sync", "tasks", "today", "w", "wq",
];

/// The module id in a Moodle assignment link, or `None` for anything that isn't an assignment.
pub fn assignment_id(url: &str) -> Option<i64> {
    if !url.contains("/mod/assign/") {
        return None;
    }
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|pair| pair.strip_prefix("id=")).and_then(|id| id.parse().ok())
}

fn eod() -> NaiveTime {
    NaiveTime::from_hms_opt(23, 59, 0).unwrap()
}

/// Case-insensitive subsequence match, so `mthw` finds `Math homework`.
pub fn fuzzy(hay: &str, needle: &str) -> bool {
    let hay = hay.to_lowercase();
    let mut chars = hay.chars();
    needle.to_lowercase().chars().all(|n| chars.any(|h| h == n))
}

impl App {
    pub fn new(cfg: Config) -> Result<App> {
        let db = Db::open(&db_path())?;
        let (tx, rx) = channel();
        let browser = Browser::new(PathBuf::from(&cfg.vault));
        let mut app = App {
            cfg,
            db,
            view: View::Today,
            mode: Mode::Normal,
            input: Vec::new(),
            cursor: 0,
            pending: None,
            count: None,
            msg: "Press ? for help, or Space to see shortcuts".into(),
            msg_kind: MsgKind::Info,
            quit: false,
            want_editor: None,
            help_open: false,
            help_scroll: 0,
            tasks: vec![],
            events: vec![],
            habits: vec![],
            grades: vec![],
            inbox: vec![],
            briefing: None,
            notes: vec![],
            focus_today: 0,
            last_sync: None,
            sel_today: 0,
            sel_tasks: 0,
            sel_notes: 0,
            sel_habits: 0,
            sel_grades: 0,
            sel_agenda: 0,
            habit_col: 0,
            cal_date: today(),
            filter: TaskFilter::All,
            show_done: false,
            show_past: false,
            search: String::new(),
            preview_scroll: 0,
            browser,
            preview: Preview::None,
            find_sel: 0,
            preview_path: None,
            form: None,
            sel_settings: 0,
            timer_next: String::new(),
            last_sync_try: Instant::now(),
            focus: None,
            chat: vec![],
            chat_scroll: 0,
            chat_busy: false,
            syncing: false,
            briefing_busy: false,
            spinner: 0,
            undo: vec![],
            history: vec![],
            history_pos: None,
            last_version: 0,
            last_reload: Instant::now(),
            tx,
            rx,
        };
        app.reload()?;
        app.scan_notes();
        Ok(app)
    }

    pub fn reload(&mut self) -> Result<()> {
        self.tasks = self.db.tasks()?;
        self.events = self.db.events()?;
        self.habits = self.db.habits()?;
        self.grades = self.db.grades()?;
        self.inbox = self.db.inbox()?;
        self.briefing = self.db.latest_briefing()?;
        self.focus_today = self.db.focus_minutes_today()?;
        self.last_sync = self.db.meta("last_sync").and_then(|v| v.parse().ok());
        self.last_version = self.db.data_version();
        self.clamp();
        Ok(())
    }

    /// Work that shouldn't happen while tests build an App.
    pub fn startup(&mut self) {
        if self.cfg.sync_on_open {
            self.start_sync();
        }
    }

    pub fn scan_notes(&mut self) {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<NoteEntry>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    walk(&path, root, out);
                } else if path.extension().is_some_and(|e| e == "md") {
                    let modified = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_secs() as i64);
                    let rel = path.strip_prefix(root).unwrap_or(&path).with_extension("").display().to_string();
                    out.push(NoteEntry { path, rel, modified });
                }
            }
        }
        let root = PathBuf::from(&self.cfg.vault);
        let mut notes = Vec::new();
        walk(&root, &root, &mut notes);
        notes.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.rel.cmp(&b.rel)));
        self.notes = notes;
        self.browser.refresh();
        self.preview_path = None;
        self.clamp();
    }

    // ---- derived lists ----------------------------------------------------

    pub fn visible_tasks(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| self.show_done || !t.done)
            .filter(|t| match self.filter {
                TaskFilter::All => true,
                TaskFilter::Moodle => t.is_moodle(),
                TaskFilter::Mine => !t.is_moodle(),
            })
            .filter(|t| self.search.is_empty() || fuzzy(&format!("{} {}", t.title, t.course), &self.search))
            .collect()
    }

    /// Overdue work, anything due in the next three days, and your high-priority tasks.
    pub fn today_tasks(&self) -> Vec<&Task> {
        let horizon = ts_of(today() + Duration::days(3), eod());
        self.tasks.iter().filter(|t| !t.done && (t.due.is_some_and(|d| d <= horizon) || t.priority >= 2)).collect()
    }

    /// The current folder in the Notes tab, narrowed by `/`.
    pub fn visible_entries(&self) -> Vec<&Entry> {
        self.browser.entries.iter().filter(|e| self.search.is_empty() || fuzzy(&e.name, &self.search)).collect()
    }

    /// Notes anywhere in the vault that match the find prompt.
    pub fn find_matches(&self) -> Vec<&NoteEntry> {
        let query = self.input_text();
        self.notes.iter().filter(|n| fuzzy(&n.rel, &query)).take(200).collect()
    }

    pub fn is_finding(&self) -> bool {
        self.mode == Mode::Input(Prompt::VaultFind)
    }

    /// What the preview column should show: the find result while finding, otherwise the selection.
    fn preview_target(&self) -> Option<Entry> {
        if self.is_finding() {
            let hit = *self.find_matches().get(self.find_sel)?;
            let name = hit.path.file_name()?.to_string_lossy().to_string();
            return Some(Entry { name, path: hit.path.clone(), is_dir: false, size: 0, modified: hit.modified });
        }
        self.visible_entries().get(self.sel_notes).map(|e| (*e).clone())
    }

    pub fn visible_grades(&self) -> Vec<&Grade> {
        self.grades.iter().filter(|g| self.show_past || g.current).collect()
    }

    pub fn agenda(&self, day: NaiveDate) -> Vec<AgendaItem> {
        let mut items: Vec<AgendaItem> = self
            .tasks
            .iter()
            .filter(|t| !t.done && t.due.is_some_and(|d| date_of(d) == day))
            .map(|t| AgendaItem::Task(t.clone()))
            .chain(self.events.iter().filter(|e| date_of(e.start) == day).map(|e| AgendaItem::Event(e.clone())))
            .collect();
        items.sort_by_key(|i| i.time());
        items
    }

    pub fn current_task(&self) -> Option<Task> {
        match self.view {
            View::Today => self.today_tasks().get(self.sel_today).map(|t| (*t).clone()),
            View::Tasks => self.visible_tasks().get(self.sel_tasks).map(|t| (*t).clone()),
            View::Calendar => match self.agenda(self.cal_date).get(self.sel_agenda) {
                Some(AgendaItem::Task(t)) => Some(t.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    fn list_len(&self) -> usize {
        match self.view {
            View::Today => self.today_tasks().len(),
            View::Tasks => self.visible_tasks().len(),
            View::Notes => self.visible_entries().len(),
            View::Habits => self.habits.len(),
            View::Grades => self.visible_grades().len(),
            View::Settings => settings::ROWS.len(),
            View::Calendar | View::Focus | View::Claude => 0,
        }
    }

    fn sel_mut(&mut self) -> Option<&mut usize> {
        match self.view {
            View::Today => Some(&mut self.sel_today),
            View::Tasks => Some(&mut self.sel_tasks),
            View::Notes => Some(&mut self.sel_notes),
            View::Habits => Some(&mut self.sel_habits),
            View::Grades => Some(&mut self.sel_grades),
            View::Settings => Some(&mut self.sel_settings),
            View::Calendar | View::Focus | View::Claude => None,
        }
    }

    fn clamp(&mut self) {
        let last = |n: usize| n.saturating_sub(1);
        self.sel_today = self.sel_today.min(last(self.today_tasks().len()));
        self.sel_tasks = self.sel_tasks.min(last(self.visible_tasks().len()));
        self.sel_notes = self.sel_notes.min(last(self.visible_entries().len()));
        self.sel_habits = self.sel_habits.min(last(self.habits.len()));
        self.sel_grades = self.sel_grades.min(last(self.visible_grades().len()));
        self.sel_agenda = self.sel_agenda.min(last(self.agenda(self.cal_date).len()));
    }

    fn move_sel(&mut self, delta: isize) {
        let len = self.list_len();
        if let Some(sel) = self.sel_mut() {
            *sel = sel.saturating_add_signed(delta).min(len.saturating_sub(1));
        }
        self.preview_scroll = 0;
    }

    fn set_sel(&mut self, value: usize) {
        let len = self.list_len();
        if let Some(sel) = self.sel_mut() {
            *sel = value.min(len.saturating_sub(1));
        }
        self.preview_scroll = 0;
    }

    // ---- per-frame work ---------------------------------------------------

    /// Refreshes the file preview before a draw, since drawing only reads state.
    pub fn prepare(&mut self) {
        if self.view != View::Notes {
            return;
        }
        let target = self.preview_target();
        let path = target.as_ref().map(|e| e.path.clone());
        if path != self.preview_path {
            self.preview = files::preview(target.as_ref(), self.browser.show_hidden, self.browser.sort);
            self.preview_path = path;
            self.preview_scroll = 0;
        }
    }

    pub fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
        while let Ok(bg) = self.rx.try_recv() {
            self.on_bg(bg);
        }
        self.tick_focus();
        self.tick_auto_sync();
        if self.last_reload.elapsed().as_secs() >= 2 {
            self.last_reload = Instant::now();
            if self.db.data_version() != self.last_version {
                self.refresh();
            }
        }
    }

    fn on_bg(&mut self, bg: Bg) {
        match bg {
            Bg::Sync(r) => {
                self.syncing = false;
                match r {
                    Ok(m) => self.info(m),
                    Err(e) => self.error(e),
                }
            }
            Bg::Brief(r) => {
                self.briefing_busy = false;
                match r {
                    Ok(m) => self.info(m),
                    Err(e) => self.error(e),
                }
            }
            Bg::Chat(r) => {
                self.chat_busy = false;
                match r {
                    Ok(answer) => self.chat.push((false, answer)),
                    Err(e) => self.chat.push((false, format!("Error: {e}"))),
                }
                self.chat_scroll = 0;
            }
        }
        self.refresh();
    }

    fn tick_auto_sync(&mut self) {
        let minutes = self.cfg.sync_interval_minutes;
        if minutes <= 0 || self.syncing {
            return;
        }
        let interval = minutes * 60;
        // Counting from the last attempt stops a failing sync from retrying every tick.
        let waited = self.last_sync_try.elapsed().as_secs() as i64 >= interval;
        let stale = self.last_sync.is_none_or(|t| now_ts() - t >= interval);
        if waited && stale {
            self.start_sync();
        }
    }

    fn tick_focus(&mut self) {
        let Some(focus) = &self.focus else { return };
        if focus.paused || focus.remaining() > 0 {
            return;
        }
        let (phase, task_id, title, minutes) = (focus.phase, focus.task_id, focus.title.clone(), focus.minutes);
        self.focus = None;
        let (head, body) = match phase {
            Phase::Work => {
                if let Err(e) = self.db.log_focus(task_id, &title, minutes) {
                    self.error(e.to_string());
                }
                ("Focus session finished".to_string(), format!("{minutes} min on {title}. Press b in LockIn for a break."))
            }
            Phase::Break => ("Break is over".to_string(), "Press s in LockIn to focus again.".to_string()),
        };
        let cfg = self.cfg.clone();
        let msg = head.clone();
        std::thread::spawn(move || {
            if cfg.desktop_notifications {
                let _ = services::desktop(&head, &body, false);
            }
        });
        self.info(msg);
        self.refresh();
    }

    pub fn refresh(&mut self) {
        if let Err(e) = self.reload() {
            self.error(e.to_string());
        }
    }

    pub fn info(&mut self, m: impl Into<String>) {
        self.msg = m.into();
        self.msg_kind = MsgKind::Info;
    }

    pub fn error(&mut self, m: impl Into<String>) {
        self.msg = m.into();
        self.msg_kind = MsgKind::Error;
    }

    fn attempt(&mut self, r: Result<()>) {
        if let Err(e) = r {
            self.error(e.to_string());
        }
        self.refresh();
    }

    // ---- background jobs --------------------------------------------------

    pub fn start_sync(&mut self) {
        if self.syncing {
            return self.info("Sync already running");
        }
        self.syncing = true;
        self.last_sync_try = Instant::now();
        self.info("Syncing Moodle…");
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = (|| -> Result<String> {
                let mut db = Db::open(&db_path())?;
                let s = services::sync(&mut db)?;
                Ok(format!("Moodle synced: {} new, {} updated, {} finished", s.added, s.updated, s.completed))
            })();
            let _ = tx.send(Bg::Sync(r.map_err(|e| e.to_string())));
        });
    }

    pub fn start_brief(&mut self) {
        if self.briefing_busy {
            return self.info("Claude is already writing the briefing");
        }
        self.briefing_busy = true;
        self.info("Claude is writing your briefing…");
        let (tx, cfg) = (self.tx.clone(), self.cfg.clone());
        std::thread::spawn(move || {
            let r = (|| -> Result<String> {
                let db = Db::open(&db_path())?;
                Ok(services::brief(&db, &cfg)?.headline)
            })();
            let _ = tx.send(Bg::Brief(r.map_err(|e| e.to_string())));
        });
    }

    fn send_chat(&mut self, text: String) {
        self.chat.push((true, text));
        self.chat_busy = true;
        self.chat_scroll = 0;
        let (tx, cfg, transcript) = (self.tx.clone(), self.cfg.clone(), self.chat.clone());
        std::thread::spawn(move || {
            let r = (|| -> Result<String> {
                let db = Db::open(&db_path())?;
                services::chat(&db, &cfg, &transcript)
            })();
            let _ = tx.send(Bg::Chat(r.map_err(|e| e.to_string())));
        });
    }

    // ---- input ------------------------------------------------------------

    fn open_prompt(&mut self, prompt: Prompt, initial: &str) {
        self.input = initial.chars().collect();
        self.cursor = self.input.len();
        self.history_pos = None;
        self.mode = Mode::Input(prompt);
    }

    pub fn input_text(&self) -> String {
        self.input.iter().collect()
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        match self.mode.clone() {
            Mode::Normal => self.on_normal(key),
            Mode::Input(p) => self.on_input(key, p),
            Mode::Confirm(c) => self.on_confirm(key, c),
            Mode::Form => self.on_form_key(key),
        }
    }

    fn on_confirm(&mut self, key: KeyEvent, c: Confirm) {
        self.mode = Mode::Normal;
        // On the calendar, a second d confirms too, so vim's dd just works.
        let second_d = key.code == KeyCode::Char('d') && matches!(c, Confirm::DeleteAgenda(_));
        if !second_d && !matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            return self.info("Cancelled");
        }
        match c {
            Confirm::DeleteAgenda(_) => self.delete_current(),
            Confirm::TrashNote(path) => {
                let r = self.trash_entry(&path);
                self.attempt(r);
                self.scan_notes();
            }
            Confirm::DeleteHabit(id, name) => {
                let r = self.db.delete_habit(id);
                self.attempt(r);
                self.info(format!("Deleted habit {name}"));
            }
            Confirm::Quit => self.quit = true,
        }
    }

    fn trash_entry(&mut self, path: &Path) -> Result<()> {
        let dest = files::trash(&PathBuf::from(&self.cfg.vault), path)?;
        self.undo.push(Undo::Move { from: dest, to: path.to_path_buf() });
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.info(format!("Moved {name} to the vault's .trash. Press u to undo"));
        Ok(())
    }

    fn on_input(&mut self, key: KeyEvent, prompt: Prompt) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                if prompt == Prompt::Search {
                    self.search.clear();
                    self.clamp();
                }
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Enter => {
                let text = self.input_text();
                self.mode = Mode::Normal;
                return self.submit(prompt, text.trim().to_string());
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.input.remove(self.cursor);
                } else if self.input.is_empty() {
                    self.mode = Mode::Normal; // like vim, backspacing out of an empty line leaves it
                    return;
                }
            }
            KeyCode::Delete if self.cursor < self.input.len() => {
                self.input.remove(self.cursor);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::Up if prompt == Prompt::VaultFind => {
                self.find_sel = self.find_sel.saturating_sub(1);
                return;
            }
            KeyCode::Down if prompt == Prompt::VaultFind => {
                self.find_sel = (self.find_sel + 1).min(self.find_matches().len().saturating_sub(1));
                return;
            }
            KeyCode::Char('p') if ctrl && prompt == Prompt::VaultFind => {
                self.find_sel = self.find_sel.saturating_sub(1);
                return;
            }
            KeyCode::Char('n') if ctrl && prompt == Prompt::VaultFind => {
                self.find_sel = (self.find_sel + 1).min(self.find_matches().len().saturating_sub(1));
                return;
            }
            KeyCode::Up if prompt == Prompt::Command => self.history_step(-1),
            KeyCode::Down if prompt == Prompt::Command => self.history_step(1),
            KeyCode::Tab if prompt == Prompt::Command => self.complete(),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.input.len(),
            KeyCode::Char('u') if ctrl => {
                self.input.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => {
                let mut start = self.cursor;
                while start > 0 && self.input[start - 1] == ' ' {
                    start -= 1;
                }
                while start > 0 && self.input[start - 1] != ' ' {
                    start -= 1;
                }
                self.input.drain(start..self.cursor);
                self.cursor = start;
            }
            KeyCode::Char(c) if !ctrl => {
                self.input.insert(self.cursor, c);
                self.cursor += 1;
            }
            _ => {}
        }
        if prompt == Prompt::Search {
            self.search = self.input_text();
            self.set_sel(0);
        }
        if prompt == Prompt::VaultFind {
            self.find_sel = 0;
        }
    }

    fn history_step(&mut self, dir: isize) {
        if self.history.is_empty() {
            return;
        }
        let next = match (self.history_pos, dir) {
            (None, -1) => Some(self.history.len() - 1),
            (None, _) => None,
            (Some(p), -1) => Some(p.saturating_sub(1)),
            (Some(p), _) if p + 1 < self.history.len() => Some(p + 1),
            (Some(_), _) => None,
        };
        self.history_pos = next;
        self.input = next.map(|p| self.history[p].chars().collect()).unwrap_or_default();
        self.cursor = self.input.len();
    }

    fn complete(&mut self) {
        let text = self.input_text();
        if text.contains(' ') {
            return;
        }
        let hits: Vec<&&str> = COMMANDS.iter().filter(|c| c.starts_with(&text)).collect();
        match hits.as_slice() {
            [] => {}
            [one] => {
                self.input = format!("{one} ").chars().collect();
                self.cursor = self.input.len();
            }
            many => {
                let list = many.iter().map(|c| **c).collect::<Vec<_>>().join("  ");
                self.info(list);
            }
        }
    }

    fn submit(&mut self, prompt: Prompt, text: String) {
        let result = match prompt {
            Prompt::Command => {
                if !text.is_empty() {
                    self.history.push(text.clone());
                }
                self.run_command(&text)
            }
            Prompt::Search => {
                self.search = text;
                self.clamp();
                Ok(())
            }
            Prompt::VaultFind => self.open_find_result(),
            Prompt::Chat if text.is_empty() => Ok(()),
            Prompt::Chat => {
                self.send_chat(text);
                Ok(())
            }
            _ if text.is_empty() && !matches!(prompt, Prompt::SetDue(_) | Prompt::Setting(settings::Key::AboutMe)) => Ok(()),
            Prompt::NewTask(day) => self.add_task(&text, day),
            Prompt::EditTitle(id) => self.edit_title(id, &text),
            Prompt::SetDue(id) => self.edit_due(id, &text),
            Prompt::NewNote => self.create_entry(&text),
            Prompt::Rename(path) => self.rename_entry(&path, &text),
            Prompt::Setting(key) => self.set_setting_text(key, &text),
            Prompt::NewHabit => self.db.add_habit(&text),
        };
        self.attempt(result);
    }

    fn push_task_undo(&mut self, id: i64) -> Result<()> {
        if let Some(t) = self.db.task(id)? {
            self.undo.push(Undo::Task(t));
        }
        Ok(())
    }

    fn add_task(&mut self, text: &str, day: Option<NaiveDate>) -> Result<()> {
        let q = parse_quick(text);
        if q.title.is_empty() {
            bail!("A task needs a title");
        }
        let due = match (&q.when, day) {
            (Some(w), _) => match parse_when(w, eod(), Local::now()) {
                Some(d) => Some(d),
                None => bail!("Couldn't read the date '{w}'. Try fri, tom 18:00, +3d or 14.11."),
            },
            (None, Some(d)) => Some(ts_of(d, eod())),
            (None, None) => None,
        };
        let id = self.db.add_task(&q.title, due, q.priority, q.course.as_deref().unwrap_or(""), q.minutes)?;
        self.undo.push(Undo::RemoveTask(id));
        self.info(format!("Added: {}", q.title));
        Ok(())
    }

    fn add_event(&mut self, text: &str) -> Result<()> {
        let q = parse_quick(text);
        if q.title.is_empty() {
            bail!("An event needs a title");
        }
        let nine = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
        let start = match &q.when {
            Some(w) => match parse_when(w, nine, Local::now()) {
                Some(s) => s,
                None => bail!("Couldn't read the date '{w}'. Try fri 14:00, tom or 14.11. 09:30"),
            },
            None => ts_of(today(), nine),
        };
        let end = start + q.minutes.unwrap_or(60) * 60;
        let id = self.db.add_event(&q.title, start, end)?;
        self.undo.push(Undo::RemoveEvent(id));
        self.cal_date = date_of(start);
        self.info(format!("Added event: {}", q.title));
        Ok(())
    }

    fn edit_title(&mut self, id: i64, text: &str) -> Result<()> {
        self.push_task_undo(id)?;
        self.db.set_title(id, text)
    }

    fn edit_due(&mut self, id: i64, text: &str) -> Result<()> {
        self.push_task_undo(id)?;
        if text.is_empty() {
            return self.db.set_due(id, None);
        }
        match parse_when(text, eod(), Local::now()) {
            Some(due) => self.db.set_due(id, Some(due)),
            None => bail!("Couldn't read the date '{text}'. Try fri, tom 18:00, +3d or 14.11."),
        }
    }

    fn new_note(&mut self, name: &str) -> Result<()> {
        let name = name.trim_end_matches(".md");
        if name.split('/').any(|p| p == ".." || p.is_empty()) {
            bail!("Use a plain name or a sub/folder/name inside the vault");
        }
        let vault = PathBuf::from(&self.cfg.vault);
        let rel = if name.contains('/') { name.to_string() } else { format!("{}/{name}", self.cfg.new_note_dir) };
        let path = vault.join(format!("{rel}.md"));
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let title = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            std::fs::write(&path, format!("# {title}\n\n"))?;
        }
        self.want_editor = Some(EditTarget::Note(path));
        Ok(())
    }

    fn create_entry(&mut self, name: &str) -> Result<()> {
        let cwd = self.browser.cwd.clone();
        let (path, open) = files::create(&cwd, name)?;
        self.scan_notes();
        // Use the real path, since `Plan` may have become `Plan.md`.
        let top = path.strip_prefix(&cwd).ok().and_then(|rel| rel.components().next()).map(|c| c.as_os_str().to_string_lossy().to_string());
        if let Some(top) = top {
            self.select_entry(&top);
        }
        if !open {
            self.info(format!("Created {name}"));
        } else if files::is_text_file(&path) {
            self.want_editor = Some(EditTarget::Note(path));
        }
        Ok(())
    }

    fn rename_entry(&mut self, path: &Path, name: &str) -> Result<()> {
        let dest = files::rename(path, name)?;
        self.undo.push(Undo::Move { from: dest, to: path.to_path_buf() });
        self.scan_notes();
        self.select_entry(name);
        Ok(())
    }

    fn select_entry(&mut self, name: &str) {
        let visible = self.visible_entries();
        self.sel_notes = self.browser.index_of(name, &visible);
    }

    fn remember_selection(&mut self) {
        if let Some(name) = self.visible_entries().get(self.sel_notes).map(|e| e.name.clone()) {
            self.browser.remember(&name);
        }
    }

    fn enter_dir(&mut self, dir: &Path) {
        self.remember_selection();
        self.search.clear();
        self.browser.enter(dir);
        self.sel_notes = 0;
        if let Some(name) = self.browser.remembered_name() {
            self.select_entry(&name);
        }
        self.preview_scroll = 0;
    }

    fn leave_dir(&mut self) {
        self.remember_selection();
        self.search.clear();
        if let Some(name) = self.browser.leave() {
            self.select_entry(&name);
        }
        self.preview_scroll = 0;
    }

    fn open_entry(&mut self) {
        let Some(entry) = self.visible_entries().get(self.sel_notes).map(|e| (*e).clone()) else { return };
        if entry.is_dir {
            self.enter_dir(&entry.path);
        } else if crate::pdf::is_pdf(&entry.path) {
            self.want_editor = Some(EditTarget::Pdf(entry.path));
        } else if files::is_text_file(&entry.path) {
            self.want_editor = Some(EditTarget::Note(entry.path));
        } else {
            self.open_external(&entry.path.display().to_string());
        }
    }

    fn open_find_result(&mut self) -> Result<()> {
        let Some(path) = self.find_matches().get(self.find_sel).map(|n| n.path.clone()) else { return Ok(()) };
        if let Some(dir) = path.parent() {
            self.enter_dir(dir);
        }
        if let Some(name) = path.file_name() {
            self.select_entry(&name.to_string_lossy());
        }
        self.want_editor = Some(EditTarget::Note(path));
        Ok(())
    }

    fn paste_yank(&mut self) -> Result<()> {
        let Some(yank) = self.browser.yank.clone() else {
            self.info("Nothing to paste. Press y to copy or x to cut");
            return Ok(());
        };
        let cwd = self.browser.cwd.clone();
        let dest = files::paste(&yank, &cwd)?;
        if yank.cut && dest != yank.path {
            self.undo.push(Undo::Move { from: dest.clone(), to: yank.path.clone() });
        }
        if yank.cut {
            self.browser.yank = None;
        }
        self.scan_notes();
        if let Some(name) = dest.file_name() {
            self.select_entry(&name.to_string_lossy());
        }
        Ok(())
    }

    /// Follows the vault's Obsidian daily-notes settings, including its template.
    fn open_daily(&mut self) -> Result<()> {
        let vault = PathBuf::from(&self.cfg.vault);
        let settings: serde_json::Value = std::fs::read_to_string(vault.join(".obsidian/daily-notes.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let folder = settings["folder"].as_str().unwrap_or(&self.cfg.new_note_dir).to_string();
        let format = settings["format"].as_str().unwrap_or("YYYY-MM-DD").replace("YYYY", "%Y").replace("MM", "%m").replace("DD", "%d");
        let name = today().format(&format).to_string();
        let path = vault.join(&folder).join(format!("{name}.md"));
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let template = settings["template"].as_str().map(|t| {
                let t = t.trim_end_matches(".md");
                vault.join(format!("{t}.md"))
            });
            let body = template.and_then(|t| std::fs::read_to_string(t).ok()).unwrap_or_default();
            let body = body.replace("{{date}}", &name).replace("{{title}}", &name).replace("{{time}}", &Local::now().format("%H:%M").to_string());
            std::fs::write(&path, body)?;
        }
        self.want_editor = Some(EditTarget::Note(path));
        Ok(())
    }

    pub fn after_editor(&mut self, target: EditTarget) {
        match target {
            EditTarget::Note(_) => self.scan_notes(),
            EditTarget::Config => match crate::config::load() {
                Ok(cfg) => {
                    self.cfg = cfg;
                    self.browser = Browser::new(PathBuf::from(&self.cfg.vault));
                    self.sel_notes = 0;
                    self.scan_notes();
                }
                Err(e) => self.error(format!("The config file has a mistake, so it wasn't applied: {e}")),
            },
            EditTarget::Pdf(_) => {}
            // You may have just submitted something, so check right away.
            EditTarget::Assignment(..) | EditTarget::MoodleApp => self.start_sync(),
            EditTarget::TaskNotes(id, tmp) => {
                let r = std::fs::read_to_string(&tmp).map_err(Into::into).and_then(|text| self.db.set_notes(id, text.trim_end()));
                let _ = std::fs::remove_file(&tmp);
                self.attempt(r);
            }
        }
        self.preview_path = None;
    }

    pub fn run_command(&mut self, line: &str) -> Result<()> {
        let (cmd, arg) = line.split_once(' ').unwrap_or((line, ""));
        let arg = arg.trim();
        match cmd {
            "" => {}
            "q" | "q!" | "qa" | "qa!" | "wq" | "x" => self.try_quit(cmd.ends_with('!')),
            "w" => self.info("Everything saves as you go"),
            "today" => self.switch(View::Today),
            "tasks" => self.switch(View::Tasks),
            "cal" | "calendar" => self.switch(View::Calendar),
            "settings" | "set" => self.switch(View::Settings),
            "moodle" => self.want_editor = Some(EditTarget::MoodleApp),
            "notes" => self.switch(View::Notes),
            "habits" => self.switch(View::Habits),
            "grades" => self.switch(View::Grades),
            "claude" => self.switch(View::Claude),
            "sync" => self.start_sync(),
            "brief" => self.start_brief(),
            "help" | "h" => self.help_open = true,
            "add" => self.add_task(arg, None)?,
            "event" => self.add_event(arg)?,
            "pdf" if arg.is_empty() => bail!("Usage: :pdf path/to/file.pdf"),
            "pdf" => {
                let path = match arg.strip_prefix("~/") {
                    Some(rest) => crate::config::home().join(rest),
                    None => PathBuf::from(arg),
                };
                if !path.is_file() {
                    bail!("{} isn't a file", path.display());
                }
                self.want_editor = Some(EditTarget::Pdf(path));
            }
            "note" if !arg.is_empty() => self.new_note(arg)?,
            "note" => bail!("Usage: :note name"),
            "daily" => self.open_daily()?,
            "habit" if !arg.is_empty() => self.db.add_habit(arg)?,
            "habit" => bail!("Usage: :habit name"),
            "focus" => {
                let minutes = if arg.is_empty() { self.cfg.focus_minutes } else { arg.parse().unwrap_or(0) };
                if minutes <= 0 {
                    bail!("Usage: :focus 25");
                }
                self.start_focus(minutes);
            }
            "stop" => self.stop_focus(),
            "filter" => {
                self.filter = match arg {
                    "moodle" => TaskFilter::Moodle,
                    "mine" => TaskFilter::Mine,
                    "all" | "" => TaskFilter::All,
                    _ => bail!("Usage: :filter all|moodle|mine"),
                };
                self.switch(View::Tasks);
            }
            "notify" => {
                let cfg = self.cfg.clone();
                self.info("Sending a test notification…");
                std::thread::spawn(move || services::notify_all(&cfg, "LockIn test", "Notifications are working.", false, false));
            }
            "config" => self.want_editor = Some(EditTarget::Config),
            other => bail!("Not a command: {other}. Press Tab to list commands"),
        }
        Ok(())
    }

    fn try_quit(&mut self, force: bool) {
        if !force && self.focus.as_ref().is_some_and(|f| f.phase == Phase::Work) {
            self.mode = Mode::Confirm(Confirm::Quit);
        } else {
            self.quit = true;
        }
    }

    fn switch(&mut self, view: View) {
        self.view = view;
        self.search.clear();
        self.preview_scroll = 0;
        match view {
            View::Notes => self.scan_notes(),
            View::Settings => self.timer_next = services::timer_next_run(),
            _ => {}
        }
        self.clamp();
    }

    fn tab_step(&mut self, delta: isize) {
        let i = VIEWS.iter().position(|(v, _)| *v == self.view).unwrap_or(0) as isize;
        let next = (i + delta).rem_euclid(VIEWS.len() as isize) as usize;
        self.switch(VIEWS[next].0);
    }

    // ---- normal mode ------------------------------------------------------

    fn on_normal(&mut self, key: KeyEvent) {
        if self.help_open {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => self.help_scroll = self.help_scroll.saturating_add(1),
                KeyCode::Char('k') | KeyCode::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
                _ => {
                    self.help_open = false;
                    self.help_scroll = 0;
                }
            }
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let KeyCode::Char(c) = key.code else {
            return self.on_special(key);
        };
        if ctrl {
            return self.on_ctrl(c);
        }
        if let Some(prefix) = self.pending.take() {
            let n = self.count.take();
            return self.on_chord(prefix, c, n);
        }
        if c.is_ascii_digit() && (c != '0' || self.count.is_some()) {
            let digit = c.to_digit(10).unwrap_or(0) as usize;
            self.count = Some((self.count.unwrap_or(0) * 10 + digit).min(9999));
            return;
        }
        let n = self.count.take();
        let times = n.unwrap_or(1) as isize;
        // Yazi's single-key delete and jump, which elsewhere start a two-key chord.
        if self.view == View::Notes && matches!(c, 'd' | 'z') {
            self.on_notes_key(c);
            return;
        }
        if self.view == View::Calendar && c == 'd' {
            return self.ask_delete_agenda();
        }
        match c {
            ':' => self.open_prompt(Prompt::Command, ""),
            '/' => self.open_prompt(Prompt::Search, ""),
            '?' => self.help_open = true,
            ' ' | 'g' | 'd' | 'z' | 'Z' => self.pending = Some(c),
            'u' => self.undo_last(),
            _ => self.on_view_key(c, times),
        }
    }

    fn on_special(&mut self, key: KeyEvent) {
        self.pending = None;
        let times = self.count.take().unwrap_or(1) as isize;
        match key.code {
            KeyCode::Tab => self.tab_step(1),
            KeyCode::BackTab => self.tab_step(-1),
            KeyCode::Esc => {
                if !self.search.is_empty() {
                    self.search.clear();
                    self.clamp();
                }
            }
            KeyCode::Down => self.on_view_key('j', times),
            KeyCode::Up => self.on_view_key('k', times),
            KeyCode::Left => self.on_view_key('h', times),
            KeyCode::Right => self.on_view_key('l', times),
            KeyCode::Enter => self.on_enter(),
            _ => {}
        }
    }

    fn on_ctrl(&mut self, c: char) {
        self.pending = None;
        self.count = None;
        match c {
            'd' => self.half_page(1),
            'u' => self.half_page(-1),
            'c' => self.info("Type :q to quit"),
            'l' => self.refresh(),
            _ => {}
        }
    }

    fn half_page(&mut self, dir: isize) {
        match self.view {
            View::Claude => self.chat_scroll = if dir > 0 { self.chat_scroll.saturating_sub(10) } else { self.chat_scroll.saturating_add(10) },
            View::Calendar => self.month_step(dir as i32),
            _ => self.move_sel(10 * dir),
        }
    }

    fn on_chord(&mut self, prefix: char, c: char, n: Option<usize>) {
        match (prefix, c) {
            ('g', 'g') => match self.view {
                View::Claude => self.chat_scroll = u16::MAX,
                _ => self.set_sel(n.map_or(0, |v| v.saturating_sub(1))),
            },
            ('g', 't') => match n {
                Some(i) if (1..=VIEWS.len()).contains(&i) => self.switch(VIEWS[i - 1].0),
                _ => self.tab_step(1),
            },
            ('g', 'T') => self.tab_step(-1),
            ('g', 's') => self.start_sync(),
            ('g', 'x') => self.open_url(),
            ('g', 'f') => self.focus_on_current(),
            ('d', 'd') => self.delete_current(),
            ('z', 'd') => {
                self.show_done = !self.show_done;
                self.clamp();
                let state = if self.show_done { "shown" } else { "hidden" };
                self.info(format!("Finished tasks {state}"));
            }
            ('z', 'p') => {
                self.show_past = !self.show_past;
                self.clamp();
                let state = if self.show_past { "shown" } else { "hidden" };
                self.info(format!("Past courses {state}"));
            }
            ('Z', 'Z') | ('Z', 'Q') => self.try_quit(false),
            (' ', 't') => self.switch(View::Today),
            (' ', 'T') => self.switch(View::Tasks),
            (' ', 'c') => self.switch(View::Calendar),
            (' ', 'n') => self.switch(View::Notes),
            (' ', 'h') => self.switch(View::Habits),
            (' ', 'g') => self.switch(View::Grades),
            (' ', 'p') => self.switch(View::Focus),
            (' ', 'a') => {
                self.switch(View::Claude);
                self.open_prompt(Prompt::Chat, "");
            }
            (' ', 'f') => {
                self.switch(View::Notes);
                self.find_sel = 0;
                self.open_prompt(Prompt::VaultFind, "");
            }
            (' ', 'm') => self.want_editor = Some(EditTarget::MoodleApp),
            (' ', ',') => self.switch(View::Settings),
            (' ', 's') => self.start_sync(),
            (' ', 'b') => self.start_brief(),
            (' ', 'd') => {
                let r = self.open_daily();
                self.attempt(r);
            }
            (' ', '?') => self.help_open = true,
            (' ', 'q') => self.try_quit(false),
            _ => {}
        }
    }

    fn on_enter(&mut self) {
        match self.view {
            View::Today => {
                if let Some(t) = self.current_task() {
                    if t.is_moodle() {
                        return self.open_in_moodle(&t);
                    }
                    self.switch(View::Tasks);
                    if let Some(i) = self.visible_tasks().iter().position(|x| x.id == t.id) {
                        self.sel_tasks = i;
                    }
                }
            }
            View::Calendar => self.edit_agenda(),
            View::Tasks => {
                let Some(t) = self.current_task() else { return };
                if t.is_moodle() {
                    self.open_in_moodle(&t);
                } else {
                    let tmp = crate::config::data_dir().join(format!("task-{}.md", t.id));
                    match std::fs::write(&tmp, &t.notes) {
                        Ok(()) => self.want_editor = Some(EditTarget::TaskNotes(t.id, tmp)),
                        Err(e) => self.error(e.to_string()),
                    }
                }
            }
            View::Notes => self.open_entry(),
            View::Settings => self.activate_setting(),
            View::Claude => self.open_prompt(Prompt::Chat, ""),
            _ => {}
        }
    }

    fn on_view_key(&mut self, c: char, times: isize) {
        match self.view {
            View::Calendar => return self.on_calendar_key(c, times),
            View::Focus => return self.on_focus_key(c),
            View::Claude => return self.on_claude_key(c, times),
            View::Habits => {
                if self.on_habit_key(c) {
                    return;
                }
            }
            View::Settings => {
                if self.on_settings_key(c) {
                    return;
                }
            }
            View::Notes => {
                if self.on_notes_key(c) {
                    return;
                }
            }
            _ => {}
        }
        match c {
            'j' => self.move_sel(times),
            'k' => self.move_sel(-times),
            'G' => self.set_sel(if times > 1 { times as usize - 1 } else { usize::MAX }),
            'J' => self.preview_scroll = self.preview_scroll.saturating_add(3),
            'K' => self.preview_scroll = self.preview_scroll.saturating_sub(3),
            _ => {
                if matches!(self.view, View::Today | View::Tasks) {
                    self.on_task_key(c);
                }
            }
        }
    }

    fn on_task_key(&mut self, c: char) {
        match c {
            'x' => self.toggle_current(),
            'o' | 'a' => self.open_prompt(Prompt::NewTask(None), ""),
            'i' | 'e' => self.edit_current_title(),
            'D' => self.edit_current_due(),
            '+' | '=' => self.bump_priority(1),
            '-' => self.bump_priority(-1),
            'f' if self.view == View::Tasks => {
                self.filter = match self.filter {
                    TaskFilter::All => TaskFilter::Moodle,
                    TaskFilter::Moodle => TaskFilter::Mine,
                    TaskFilter::Mine => TaskFilter::All,
                };
                self.clamp();
            }
            _ => {}
        }
    }

    fn toggle_current(&mut self) {
        let Some(t) = self.current_task() else { return };
        self.undo.push(Undo::Task(t.clone()));
        let r = self.db.set_done(t.id, !t.done);
        if !t.done && t.is_moodle() {
            self.info("Marked done here only. Moodle still needs your actual submission");
        }
        self.attempt(r);
    }

    fn edit_current_title(&mut self) {
        let Some(t) = self.current_task() else { return };
        if t.is_moodle() {
            return self.info("Moodle titles sync from Moodle, so edits wouldn't last");
        }
        self.open_prompt(Prompt::EditTitle(t.id), &t.title);
    }

    fn edit_current_due(&mut self) {
        let Some(t) = self.current_task() else { return };
        if t.is_moodle() {
            return self.info("Moodle deadlines sync from Moodle, so edits wouldn't last");
        }
        self.open_prompt(Prompt::SetDue(t.id), "");
    }

    fn bump_priority(&mut self, delta: i64) {
        let Some(t) = self.current_task() else { return };
        self.undo.push(Undo::Task(t.clone()));
        let r = self.db.set_priority(t.id, t.priority + delta);
        self.attempt(r);
    }

    fn delete_current(&mut self) {
        match self.view {
            View::Today | View::Tasks => self.delete_task_current(),
            View::Calendar => match self.agenda(self.cal_date).get(self.sel_agenda).cloned() {
                Some(AgendaItem::Task(_)) => self.delete_task_current(),
                Some(AgendaItem::Event(e)) => {
                    if e.source == "moodle" {
                        return self.info("This event comes from Moodle and would return on the next sync");
                    }
                    self.undo.push(Undo::Event(e.clone()));
                    let r = self.db.delete_event(e.id);
                    self.attempt(r);
                    self.info(format!("Deleted {}. Press u to undo", e.title));
                }
                None => {}
            },
            View::Habits => {
                if let Some(h) = self.habits.get(self.sel_habits) {
                    self.mode = Mode::Confirm(Confirm::DeleteHabit(h.id, h.name.clone()));
                }
            }
            _ => {}
        }
    }

    fn delete_task_current(&mut self) {
        let Some(t) = self.current_task() else { return };
        if t.is_moodle() {
            return self.info("Moodle tasks can't be deleted. Press x to mark it done here");
        }
        self.undo.push(Undo::Task(t.clone()));
        let r = self.db.delete_task(t.id);
        self.attempt(r);
        self.info(format!("Deleted {}. Press u to undo", t.title));
    }

    fn undo_last(&mut self) {
        let Some(u) = self.undo.pop() else {
            return self.info("Nothing to undo");
        };
        let r = match u {
            Undo::Task(t) => self.db.put_task(&t),
            Undo::RemoveTask(id) => self.db.delete_task(id),
            Undo::Event(e) => self.db.put_event(&e),
            Undo::RemoveEvent(id) => self.db.delete_event(id),
            Undo::Move { from, to } => {
                let moved = if to.exists() {
                    Err(anyhow::anyhow!("{} already exists", to.display()))
                } else {
                    std::fs::rename(&from, &to).map_err(Into::into)
                };
                self.scan_notes();
                if let Some(name) = to.file_name() {
                    self.select_entry(&name.to_string_lossy());
                }
                moved
            }
        };
        let failed = r.is_err();
        self.attempt(r);
        if !failed {
            self.info("Undone");
        }
    }

    /// Assignments open on the moodle CLI's own page, where you can upload. Anything else has no
    /// such page, so it opens in the browser.
    fn open_in_moodle(&mut self, t: &Task) {
        match assignment_id(&t.url) {
            Some(id) => self.want_editor = Some(EditTarget::Assignment(id, t.course.clone())),
            None => self.open_url(),
        }
    }

    fn open_url(&mut self) {
        let url = self.current_task().map(|t| t.url).unwrap_or_default();
        if url.is_empty() {
            return self.info("This task has no link");
        }
        self.open_external(&url);
    }

    fn open_external(&mut self, target: &str) {
        let opened = std::process::Command::new("xdg-open")
            .arg(target)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match opened {
            Ok(_) => self.info("Opened with your default app"),
            Err(e) => self.error(format!("Couldn't open it: {e}")),
        }
    }

    fn on_calendar_key(&mut self, c: char, times: isize) {
        let days = |n: isize| Duration::days(n as i64);
        match c {
            'h' => self.cal_date -= days(times),
            'l' => self.cal_date += days(times),
            'j' => self.cal_date += days(7 * times),
            'k' => self.cal_date -= days(7 * times),
            'H' | '[' => self.month_step(-(times as i32)),
            'L' | ']' => self.month_step(times as i32),
            't' => self.cal_date = today(),
            'J' | 'n' => self.sel_agenda = (self.sel_agenda + 1).min(self.agenda(self.cal_date).len().saturating_sub(1)),
            'K' | 'N' => self.sel_agenda = self.sel_agenda.saturating_sub(1),
            'o' => self.open_form(Form::new_event(self.cal_date)),
            'a' => self.open_form(Form::new_task(self.cal_date)),
            'e' | 'i' => self.edit_agenda(),
            '+' | '=' => self.shift_agenda(times as i64),
            '-' => self.shift_agenda(-(times as i64)),
            'x' => self.toggle_current(),
            _ => return,
        }
        if matches!(c, 'h' | 'l' | 'j' | 'k' | 'H' | 'L' | '[' | ']' | 't') {
            self.sel_agenda = 0;
        }
    }

    // ---- calendar editing ---------------------------------------------------

    fn open_form(&mut self, form: Form) {
        self.form = Some(form);
        self.mode = Mode::Form;
    }

    fn selected_agenda(&self) -> Option<AgendaItem> {
        self.agenda(self.cal_date).get(self.sel_agenda).cloned()
    }

    /// Edits the selected item. On an empty day, Enter simply starts a new event.
    fn edit_agenda(&mut self) {
        match self.selected_agenda() {
            None => self.open_form(Form::new_event(self.cal_date)),
            Some(AgendaItem::Task(t)) if t.is_moodle() => self.open_in_moodle(&t),
            Some(AgendaItem::Task(t)) => self.open_form(Form::edit_task(&t, self.cal_date)),
            Some(AgendaItem::Event(e)) if e.source == "moodle" => self.info("This event comes from Moodle, so change it there"),
            Some(AgendaItem::Event(e)) => self.open_form(Form::edit_event(&e)),
        }
    }

    fn ask_delete_agenda(&mut self) {
        match self.selected_agenda() {
            None => self.info("Nothing to delete on this day"),
            Some(AgendaItem::Task(t)) if t.is_moodle() => self.info("Moodle tasks can't be deleted. Press x to mark it done here"),
            Some(AgendaItem::Event(e)) if e.source == "moodle" => self.info("This event comes from Moodle and would return on the next sync"),
            Some(item) => self.mode = Mode::Confirm(Confirm::DeleteAgenda(item.title().to_string())),
        }
    }

    fn select_agenda_item(&mut self, target: Target) {
        let position = self.agenda(self.cal_date).iter().position(|item| match (item, target) {
            (AgendaItem::Event(e), Target::Event(Some(id))) => e.id == id,
            (AgendaItem::Task(t), Target::Task(Some(id))) => t.id == id,
            _ => false,
        });
        self.sel_agenda = position.unwrap_or(0);
    }

    /// Moves the selected item by whole days, with the calendar following it.
    fn shift_agenda(&mut self, days: i64) {
        let (r, target, new_time) = match self.selected_agenda() {
            None => return self.info("Nothing selected on this day"),
            Some(AgendaItem::Task(t)) if t.is_moodle() => return self.info("Moodle deadlines sync from Moodle, so they can't be moved"),
            Some(AgendaItem::Event(e)) if e.source == "moodle" => return self.info("This event comes from Moodle, so change it there"),
            Some(AgendaItem::Task(t)) => {
                let due = form::shift_days(t.due.unwrap_or_else(now_ts), days);
                self.undo.push(Undo::Task(t.clone()));
                (self.db.set_due(t.id, Some(due)), Target::Task(Some(t.id)), due)
            }
            Some(AgendaItem::Event(e)) => {
                let start = form::shift_days(e.start, days);
                let moved = Event { start, end: start + (e.end - e.start), ..e.clone() };
                self.undo.push(Undo::Event(e));
                (self.db.put_event(&moved), Target::Event(Some(moved.id)), start)
            }
        };
        self.attempt(r);
        self.cal_date = date_of(new_time);
        self.select_agenda_item(target);
        self.info(format!("Moved to {}. Press u to undo", self.cal_date.format("%a %d %b")));
    }

    fn on_form_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(f) = self.form.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        f.error = None;
        match key.code {
            KeyCode::Esc => {
                self.form = None;
                self.mode = Mode::Normal;
                self.info("Cancelled");
            }
            KeyCode::Enter => self.save_form(),
            KeyCode::Tab | KeyCode::Down => f.step(1),
            KeyCode::BackTab | KeyCode::Up => f.step(-1),
            KeyCode::Left => f.left(),
            KeyCode::Right => f.right(),
            KeyCode::Backspace => f.backspace(),
            KeyCode::Char('t') if ctrl => f.toggle_type(),
            KeyCode::Char('u') if ctrl => f.clear_line(),
            KeyCode::Char(c) if !ctrl => f.insert(c),
            _ => {}
        }
    }

    fn save_form(&mut self) {
        let Some(mut f) = self.form.take() else { return };
        let parsed = match f.parse() {
            Ok(p) => p,
            Err(e) => {
                // Keep the form open with what you typed, on the field that needs fixing.
                f.error = Some(e.to_string());
                self.form = Some(f);
                return;
            }
        };
        self.mode = Mode::Normal;
        let result = self.write_form(f.target, &parsed);
        match result {
            Ok(target) => {
                self.refresh();
                self.cal_date = date_of(parsed.start);
                self.select_agenda_item(target);
                let verb = if f.target.is_new() { "Added" } else { "Saved" };
                self.info(format!("{verb} {} on {}. Press u to undo", parsed.title, self.cal_date.format("%a %d %b")));
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    fn write_form(&mut self, target: Target, p: &form::Parsed) -> Result<Target> {
        match target {
            Target::Event(None) => {
                let id = self.db.add_event(&p.title, p.start, p.end)?;
                let saved = Event { id, title: p.title.clone(), start: p.start, end: p.end, location: p.location.clone(), notes: p.notes.clone(), source: "local".into(), ..Default::default() };
                self.db.put_event(&saved)?;
                self.undo.push(Undo::RemoveEvent(id));
                Ok(Target::Event(Some(id)))
            }
            Target::Event(Some(id)) => {
                let Some(old) = self.events.iter().find(|e| e.id == id).cloned() else { bail!("That event no longer exists") };
                let saved = Event { title: p.title.clone(), start: p.start, end: p.end, location: p.location.clone(), notes: p.notes.clone(), ..old.clone() };
                self.db.put_event(&saved)?;
                self.undo.push(Undo::Event(old));
                Ok(target)
            }
            Target::Task(None) => {
                let id = self.db.add_task(&p.title, Some(p.start), 0, "", None)?;
                self.undo.push(Undo::RemoveTask(id));
                Ok(Target::Task(Some(id)))
            }
            Target::Task(Some(id)) => {
                self.push_task_undo(id)?;
                self.db.set_title(id, &p.title)?;
                self.db.set_due(id, Some(p.start))?;
                Ok(target)
            }
        }
    }

    fn month_step(&mut self, delta: i32) {
        let months = Months::new(delta.unsigned_abs());
        let moved = if delta >= 0 { self.cal_date.checked_add_months(months) } else { self.cal_date.checked_sub_months(months) };
        if let Some(d) = moved {
            self.cal_date = d;
        }
        self.sel_agenda = 0;
    }

    fn on_notes_key(&mut self, c: char) -> bool {
        let selected = self.visible_entries().get(self.sel_notes).map(|e| (*e).clone());
        match c {
            'h' => self.leave_dir(),
            'l' => self.open_entry(),
            'a' | 'o' => self.open_prompt(Prompt::NewNote, ""),
            'r' => {
                if let Some(e) = selected {
                    self.open_prompt(Prompt::Rename(e.path), &e.name);
                }
            }
            'd' => {
                if let Some(e) = selected {
                    self.mode = Mode::Confirm(Confirm::TrashNote(e.path));
                }
            }
            'y' | 'x' => {
                if let Some(e) = selected {
                    let cut = c == 'x';
                    self.browser.yank = Some(Yank { path: e.path, cut });
                    let verb = if cut { "Cut" } else { "Copied" };
                    self.info(format!("{verb} {}. Go to a folder and press p", e.name));
                }
            }
            'p' => {
                let r = self.paste_yank();
                self.attempt(r);
            }
            'Y' | 'X' => {
                self.browser.yank = None;
                self.info("Cleared the copied item");
            }
            '.' => {
                self.browser.show_hidden = !self.browser.show_hidden;
                self.scan_notes();
            }
            's' => {
                self.browser.sort = match self.browser.sort {
                    Sort::Name => Sort::Modified,
                    Sort::Modified => Sort::Size,
                    Sort::Size => Sort::Name,
                };
                self.scan_notes();
            }
            'z' => {
                self.find_sel = 0;
                self.open_prompt(Prompt::VaultFind, "");
            }
            'D' => {
                let r = self.open_daily();
                self.attempt(r);
            }
            '~' => {
                let root = self.browser.root.clone();
                self.enter_dir(&root);
            }
            'R' => {
                self.scan_notes();
                self.info(format!("{} notes in the vault", self.notes.len()));
            }
            _ => return false,
        }
        true
    }

    // ---- settings ---------------------------------------------------------

    fn current_setting(&self) -> settings::Key {
        settings::ROWS[self.sel_settings.min(settings::ROWS.len() - 1)].1
    }

    fn on_settings_key(&mut self, c: char) -> bool {
        match c {
            'x' => self.activate_setting(),
            'l' | '+' | '=' => self.step_setting(1),
            'h' | '-' => self.step_setting(-1),
            _ => return false,
        }
        true
    }

    fn step_setting(&mut self, dir: i64) {
        let key = self.current_setting();
        if matches!(settings::kind(key), Kind::Toggle | Kind::Choice) {
            settings::step(&mut self.cfg, key, dir);
            self.commit_setting(key);
        }
    }

    fn activate_setting(&mut self) {
        let key = self.current_setting();
        use settings::Key::*;
        match settings::kind(key) {
            Kind::Toggle | Kind::Choice => self.step_setting(1),
            Kind::Text => {
                let current = settings::text_value(&self.cfg, key);
                self.open_prompt(Prompt::Setting(key), &current);
            }
            Kind::Action => match key {
                RunDaily => {
                    let r = services::run_daily_now();
                    if r.is_ok() {
                        self.info("Daily job started. Notifications will arrive shortly");
                    }
                    self.attempt(r);
                }
                SyncNow => self.start_sync(),
                TestNotify => {
                    let r = self.run_command("notify");
                    self.attempt(r);
                }
                EditFile => self.want_editor = Some(EditTarget::Config),
                _ => {}
            },
        }
    }

    fn set_setting_text(&mut self, key: settings::Key, text: &str) -> Result<()> {
        settings::set_text(&mut self.cfg, key, text)?;
        self.commit_setting(key);
        Ok(())
    }

    /// Saves a change right away, and rewrites the systemd timer when the schedule changed.
    fn commit_setting(&mut self, key: settings::Key) {
        let mut result = crate::config::save(&self.cfg);
        if result.is_ok() && settings::touches_timer(key) {
            result = services::apply_timer(&self.cfg);
            self.timer_next = services::timer_next_run();
        }
        if key == settings::Key::Vault {
            self.browser = Browser::new(PathBuf::from(&self.cfg.vault));
            self.sel_notes = 0;
            self.scan_notes();
        }
        match result {
            Ok(()) => self.info(format!("{}: {}", settings::label(key), settings::value(&self.cfg, key))),
            Err(e) => self.error(format!("Saved, but the schedule wasn't updated: {e}")),
        }
    }

    fn on_habit_key(&mut self, c: char) -> bool {
        match c {
            'o' | 'a' => self.open_prompt(Prompt::NewHabit, ""),
            'x' => {
                if let Some(h) = self.habits.get(self.sel_habits) {
                    let (id, day) = (h.id, today() - Duration::days(self.habit_col));
                    let r = self.db.toggle_habit(id, day);
                    self.attempt(r);
                }
            }
            'h' => self.habit_col = (self.habit_col + 1).min(13),
            'l' => self.habit_col = (self.habit_col - 1).max(0),
            _ => return false,
        }
        true
    }

    fn on_claude_key(&mut self, c: char, times: isize) {
        match c {
            'i' | 'a' | 'o' => self.open_prompt(Prompt::Chat, ""),
            'j' => self.chat_scroll = self.chat_scroll.saturating_sub(times.max(1) as u16),
            'k' => self.chat_scroll = self.chat_scroll.saturating_add(times.max(1) as u16),
            'G' => self.chat_scroll = 0,
            'c' => {
                self.chat.clear();
                self.chat_scroll = 0;
                self.info("Chat cleared");
            }
            _ => {}
        }
    }

    // ---- focus ------------------------------------------------------------

    fn focus_on_current(&mut self) {
        let Some(t) = self.current_task() else {
            return self.info("Select a task first");
        };
        self.focus = Some(Focus {
            phase: Phase::Work,
            task_id: Some(t.id),
            title: t.title,
            minutes: self.cfg.focus_minutes,
            started: Instant::now(),
            banked: 0,
            paused: false,
        });
        self.switch(View::Focus);
    }

    pub fn start_focus(&mut self, minutes: i64) {
        let (task_id, title) = self.current_task().map_or((None, "Focus".to_string()), |t| (Some(t.id), t.title));
        self.focus = Some(Focus { phase: Phase::Work, task_id, title, minutes, started: Instant::now(), banked: 0, paused: false });
        self.switch(View::Focus);
    }

    fn stop_focus(&mut self) {
        if self.focus.take().is_some() {
            self.info("Timer stopped. Unfinished sessions aren't logged");
        }
    }

    fn on_focus_key(&mut self, c: char) {
        match c {
            's' => {
                if self.focus.is_none() {
                    let minutes = self.cfg.focus_minutes;
                    self.start_focus(minutes);
                }
            }
            'b' => {
                self.focus = Some(Focus {
                    phase: Phase::Break,
                    task_id: None,
                    title: "Break".into(),
                    minutes: self.cfg.break_minutes,
                    started: Instant::now(),
                    banked: 0,
                    paused: false,
                });
            }
            'p' => {
                if let Some(f) = &mut self.focus {
                    if f.paused {
                        f.started = Instant::now();
                    } else {
                        f.banked += f.started.elapsed().as_secs();
                    }
                    f.paused = !f.paused;
                }
            }
            'x' => self.stop_focus(),
            '+' | '=' => {
                if let Some(f) = &mut self.focus {
                    f.minutes += 5;
                }
            }
            '-' => {
                if let Some(f) = &mut self.focus {
                    f.minutes = (f.minutes - 5).max(1);
                }
            }
            _ => {}
        }
    }
}

const OPEN_ASSIGNMENT: &str = include_str!("open_assignment.py");

/// Makes the moodle CLI open files through LockIn, which keeps PDFs in the terminal.
fn with_opener(mut command: std::process::Command) -> std::process::Command {
    if let Ok(exe) = std::env::current_exe() {
        let quoted = exe.display().to_string().replace('\'', "'\\''");
        command.env("MOODLE_OPENER", format!("'{quoted}' open"));
    }
    command
}

pub fn run_external(cfg: &Config, target: &EditTarget) -> Result<()> {
    if let EditTarget::Pdf(path) = target {
        return crate::pdf::view(path);
    }
    let mut command = match target {
        EditTarget::Pdf(_) => unreachable!("handled above"),
        EditTarget::Note(path) | EditTarget::TaskNotes(_, path) => editor_command(cfg, path),
        EditTarget::Config => editor_command(cfg, &config_path()),
        EditTarget::MoodleApp => with_opener(std::process::Command::new(find_bin("moodle"))),
        EditTarget::Assignment(id, course) => {
            let mut c = std::process::Command::new(find_bin("python3"));
            c.args(["-I", "-c", OPEN_ASSIGNMENT]).arg(find_bin("moodle")).arg(id.to_string()).arg(course);
            with_opener(c)
        }
    };
    let status = command.status()?;
    if !status.success() {
        bail!("{} exited with an error", command.get_program().to_string_lossy());
    }
    Ok(())
}

fn editor_command(cfg: &Config, path: &Path) -> std::process::Command {
    let mut parts = cfg.editor.split_whitespace();
    let mut command = std::process::Command::new(parts.next().unwrap_or("nvim"));
    command.args(parts).arg(path);
    command
}

pub fn month_first_cell(date: NaiveDate) -> NaiveDate {
    let first = date.with_day(1).unwrap_or(date);
    first - Duration::days(first.weekday().num_days_from_monday() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matches_in_order_ignoring_case() {
        assert!(fuzzy("School/Math Homework", "mthw"));
        assert!(fuzzy("Übung", "üb"));
        assert!(!fuzzy("Math", "htm"));
        assert!(fuzzy("anything", ""));
    }

    #[test]
    fn assignment_links_give_their_id() {
        let base = "https://x/moodle/mod/assign/view.php";
        assert_eq!(assignment_id(&format!("{base}?id=237070&action=editsubmission")), Some(237070));
        assert_eq!(assignment_id(&format!("{base}?action=editsubmission&id=5")), Some(5));
        assert_eq!(assignment_id("https://x/moodle/mod/quiz/view.php?id=5"), None);
        assert_eq!(assignment_id(base), None);
        assert_eq!(assignment_id(""), None);
    }

    #[test]
    fn calendar_grid_starts_on_monday() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
        assert_eq!(month_first_cell(d), NaiveDate::from_ymd_opt(2026, 10, 26).unwrap());
    }
}
