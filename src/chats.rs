//! The Chats tab: chats you create yourself, each with its own list of files, and your books.

use crate::app::{App, Bg, Confirm, EditTarget, Mode, Prompt};
use crate::books::{self, RemoteBook};
use crate::config::{self, db_path};
use crate::db::Db;
use crate::files::{self, Browser, Entry, Preview, Sort};
use crate::tutor::{self, ChatRow, Role};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Level {
    List,
    Chat,
    Files,
    Picker,
    Books,
}

pub struct BookJob {
    pub title: String,
    pub status: String,
    pub cancel: Arc<AtomicBool>,
}

pub struct ChatsState {
    pub level: Level,
    pub list: Vec<ChatRow>,
    pub sel: usize,
    pub session: Option<i64>,
    pub title: String,
    pub docs: Vec<PathBuf>,
    pub log: Vec<(Role, String)>,
    pub scroll: u16,
    pub busy: bool,
    pub file_sel: usize,
    pub browser: Option<Browser>,
    pub root_sel: usize,
    pub entry_sel: usize,
    pub preview: Preview,
    pub preview_path: Option<PathBuf>,
    pub remote: Vec<RemoteBook>,
    pub local: Vec<PathBuf>,
    pub book_sel: usize,
    /// Kept in memory only, for this run of the app, and never written anywhere.
    pub password: Option<String>,
    /// The password in memory came from storage, so a rejected login should clear it there too.
    pub password_saved: bool,
    pub asked_to_save: bool,
    pub listing: bool,
    pub job: Option<BookJob>,
}

impl Default for ChatsState {
    fn default() -> Self {
        ChatsState {
            level: Level::List,
            list: vec![],
            sel: 0,
            session: None,
            title: String::new(),
            docs: vec![],
            log: vec![],
            scroll: 0,
            busy: false,
            file_sel: 0,
            browser: None,
            root_sel: 0,
            entry_sel: 0,
            preview: Preview::None,
            preview_path: None,
            remote: vec![],
            local: vec![],
            book_sel: 0,
            password: None,
            password_saved: false,
            asked_to_save: false,
            listing: false,
            job: None,
        }
    }
}

/// One row on the Books screen: a downloaded file, or a book on your account.
pub enum BookRow<'a> {
    Local(&'a PathBuf),
    Remote(&'a RemoteBook),
}

impl App {
    pub fn book_rows(&self) -> Vec<BookRow<'_>> {
        let have = |r: &RemoteBook| books::books_dir().join(books::safe_name(&r.title)).is_dir();
        let local = self.chats.local.iter().map(BookRow::Local);
        let remote = self.chats.remote.iter().filter(move |r| !have(r)).map(BookRow::Remote);
        local.chain(remote).collect()
    }

    pub fn picker_roots(&self) -> Vec<(&'static str, PathBuf)> {
        let courses = tutor::courses(&self.db);
        let moodle = courses
            .first()
            .and_then(|c| PathBuf::from(&c.folder).parent().map(Path::to_path_buf))
            .unwrap_or_else(|| config::home().join("Moodle"));
        vec![("Books", books::books_dir()), ("Moodle downloads", moodle), ("Notes", PathBuf::from(&self.cfg.vault))]
    }

    pub fn picker_entries(&self) -> Vec<&Entry> {
        let Some(b) = &self.chats.browser else { return vec![] };
        b.entries.iter().filter(|e| self.search.is_empty() || crate::app::fuzzy(&e.name, &self.search)).collect()
    }

    pub fn chats_len(&self) -> usize {
        match self.chats.level {
            Level::List => self.chats.list.len(),
            Level::Chat => 0,
            Level::Files => self.chats.docs.len(),
            Level::Picker if self.chats.browser.is_some() => self.picker_entries().len(),
            Level::Picker => 3,
            Level::Books => self.book_rows().len(),
        }
    }

    pub fn chats_sel_mut(&mut self) -> Option<&mut usize> {
        match self.chats.level {
            Level::List => Some(&mut self.chats.sel),
            Level::Chat => None,
            Level::Files => Some(&mut self.chats.file_sel),
            Level::Picker if self.chats.browser.is_some() => Some(&mut self.chats.entry_sel),
            Level::Picker => Some(&mut self.chats.root_sel),
            Level::Books => Some(&mut self.chats.book_sel),
        }
    }

    pub fn clamp_chats(&mut self) {
        let last = |n: usize| n.saturating_sub(1);
        self.chats.sel = self.chats.sel.min(last(self.chats.list.len()));
        self.chats.file_sel = self.chats.file_sel.min(last(self.chats.docs.len()));
        self.chats.entry_sel = self.chats.entry_sel.min(last(self.picker_entries().len()));
        self.chats.root_sel = self.chats.root_sel.min(2);
        self.chats.book_sel = self.chats.book_sel.min(last(self.book_rows().len()));
    }

    pub fn reload_chats(&mut self) {
        if let Ok(list) = tutor::list_chats(&self.db) {
            self.chats.list = list;
        }
        if let Some(id) = self.chats.session {
            if let Ok(log) = tutor::messages(&self.db, id) {
                self.chats.log = log;
            }
            if let Ok(s) = tutor::session(&self.db, id) {
                self.chats.docs = s.docs;
                self.chats.title = s.title;
            }
        }
    }

    /// What the picker's preview column shows, refreshed before each draw.
    pub fn prepare_chats(&mut self) {
        let target = if self.chats.level == Level::Picker {
            self.picker_entries().get(self.chats.entry_sel).map(|e| (*e).clone())
        } else {
            None
        };
        let path = target.as_ref().map(|e| e.path.clone());
        if path != self.chats.preview_path {
            self.chats.preview = files::preview(target.as_ref(), false, Sort::Name);
            self.chats.preview_path = path;
            self.preview_scroll = 0;
        }
    }

    pub fn chats_enter(&mut self) {
        match self.chats.level {
            Level::List => {
                if let Some(id) = self.chats.list.get(self.chats.sel).map(|c| c.id) {
                    self.open_chat(id);
                } else {
                    self.open_prompt(Prompt::ChatTitle(None), "");
                }
            }
            Level::Chat => self.open_prompt(Prompt::ChatAsk, ""),
            Level::Files => self.view_chat_file(),
            Level::Picker => self.picker_open(),
            Level::Books => self.books_enter(),
        }
    }

    fn open_chat(&mut self, id: i64) {
        self.chats.session = Some(id);
        self.chats.level = Level::Chat;
        self.chats.scroll = 0;
        self.reload_chats();
    }

    pub fn chats_back(&mut self) {
        self.search.clear();
        match self.chats.level {
            Level::List => {}
            Level::Chat | Level::Books => {
                self.chats.level = Level::List;
                self.chats.session = None;
            }
            Level::Files => self.chats.level = Level::Chat,
            Level::Picker => {
                let at_top = self.chats.browser.as_ref().is_some_and(|b| b.parent_dir().is_none());
                if at_top {
                    self.chats.browser = None;
                } else if let Some(b) = self.chats.browser.as_mut() {
                    let came_from = b.leave();
                    let found = came_from.and_then(|n| self.picker_entries().iter().position(|e| e.name == n));
                    self.chats.entry_sel = found.unwrap_or(0);
                } else {
                    self.chats.level = Level::Files;
                }
            }
        }
        self.clamp_chats();
    }

    pub fn on_chats_key(&mut self, c: char, times: isize) -> bool {
        match (self.chats.level, c) {
            (Level::List, 'o' | 'a') => self.open_prompt(Prompt::ChatTitle(None), ""),
            (Level::List, 'r') => {
                if let Some(chat) = self.chats.list.get(self.chats.sel).cloned() {
                    self.open_prompt(Prompt::ChatTitle(Some(chat.id)), &chat.title);
                }
            }
            (Level::List, 'B') => self.open_books(),
            (Level::List, 'l') => self.chats_enter(),
            (Level::Chat, 'i' | 'a' | 'o') => self.open_prompt(Prompt::ChatAsk, ""),
            (Level::Chat, 'f') => {
                self.chats.level = Level::Files;
                self.chats.file_sel = 0;
            }
            (Level::Chat, 'j') => self.chats.scroll = self.chats.scroll.saturating_sub(times.max(1) as u16),
            (Level::Chat, 'k') => self.chats.scroll = self.chats.scroll.saturating_add(times.max(1) as u16),
            (Level::Chat, 'G') => self.chats.scroll = 0,
            (Level::Files, 'a' | 'o') => self.open_picker(),
            (Level::Files, 'v') => self.view_chat_file(),
            (Level::Picker, 'l') => self.picker_open(),
            (Level::Picker, 'x') => self.picker_toggle(),
            (Level::Books, 'L') => self.books_login(),
            (Level::Books, 'x') => self.cancel_book(),
            (_, 'h') => self.chats_back(),
            _ => return false,
        }
        true
    }

    /// `d` on the list, the attached files and the books. Each is single-key, like the Notes tab.
    pub fn chats_delete(&mut self) {
        match self.chats.level {
            Level::List => {
                if let Some(chat) = self.chats.list.get(self.chats.sel) {
                    self.mode = Mode::Confirm(Confirm::DeleteChat(chat.id, chat.title.clone()));
                }
            }
            Level::Files => {
                let (Some(id), Some(path)) = (self.chats.session, self.chats.docs.get(self.chats.file_sel).cloned()) else { return };
                let r = tutor::set_doc(&self.db, id, &path, false);
                self.attempt(r);
                self.info("Removed from this chat. The file itself is untouched");
            }
            Level::Books => {
                if let Some(BookRow::Local(path)) = self.book_rows().get(self.chats.book_sel) {
                    self.mode = Mode::Confirm(Confirm::DeleteBook((*path).clone()));
                }
            }
            _ => {}
        }
    }

    pub fn create_or_rename_chat(&mut self, id: Option<i64>, text: &str) -> Result<()> {
        match id {
            Some(id) if !text.is_empty() => tutor::rename_chat(&self.db, id, text),
            Some(_) => Ok(()),
            None => {
                let title = if text.is_empty() { chrono::Local::now().format("Chat %d %b %H:%M").to_string() } else { text.to_string() };
                let id = tutor::create_chat(&self.db, &title)?;
                self.open_chat(id);
                self.chats.level = Level::Files;
                self.info("Press a to add files, or h to start chatting without any");
                Ok(())
            }
        }
    }

    pub fn ask_chat(&mut self, text: String) -> Result<()> {
        let Some(id) = self.chats.session else { return Ok(()) };
        if self.chats.busy {
            bail!("Claude is still answering");
        }
        tutor::add_message(&self.db, id, Role::You, &text)?;
        self.chats.scroll = 0;
        self.chats.busy = true;
        if cfg!(test) {
            return Ok(()); // tests must never start a real, billed Claude request
        }
        let (tx, cfg) = (self.tx.clone(), self.cfg.clone());
        std::thread::spawn(move || {
            let r = Db::open(&db_path()).and_then(|db| tutor::teach(&db, &cfg, id, &text));
            let _ = tx.send(Bg::Chats(r.map_err(|e| e.to_string())));
        });
        Ok(())
    }

    fn view_chat_file(&mut self) {
        match self.chats.docs.get(self.chats.file_sel).cloned() {
            Some(p) if crate::pdf::is_pdf(&p) => self.want_editor = Some(EditTarget::Pdf(p)),
            Some(p) => self.want_editor = Some(EditTarget::Note(p)),
            None => self.info("No files yet. Press a to add one"),
        }
    }

    // ---- file picker ------------------------------------------------------

    fn open_picker(&mut self) {
        self.chats.level = Level::Picker;
        self.chats.browser = None;
        self.chats.root_sel = 0;
        self.search.clear();
    }

    fn picker_open(&mut self) {
        match &self.chats.browser {
            None => {
                let Some((_, root)) = self.picker_roots().get(self.chats.root_sel).cloned() else { return };
                if !root.is_dir() {
                    return self.info("Nothing there yet");
                }
                self.chats.browser = Some(Browser::new(root));
                self.chats.entry_sel = 0;
            }
            Some(_) => {
                let Some(e) = self.picker_entries().get(self.chats.entry_sel).map(|e| (*e).clone()) else { return };
                if e.is_dir {
                    self.search.clear();
                    if let Some(b) = self.chats.browser.as_mut() {
                        b.enter(&e.path);
                    }
                    self.chats.entry_sel = 0;
                } else {
                    self.picker_toggle();
                }
            }
        }
    }

    fn picker_toggle(&mut self) {
        let Some(e) = self.picker_entries().get(self.chats.entry_sel).map(|e| (*e).clone()) else { return };
        let Some(id) = self.chats.session else { return };
        if e.is_dir {
            return self.info("Pick files one at a time, so Claude only sees what you choose");
        }
        if !tutor::is_material_file(&e.path) {
            return self.info("Only PDFs and text files can be added");
        }
        let attach = !self.chats.docs.contains(&e.path);
        let r = tutor::set_doc(&self.db, id, &e.path, attach);
        self.attempt(r);
        let verb = if attach { "Added" } else { "Removed" };
        self.info(format!("{verb} {}. {} file(s) in this chat. h goes back", e.name, self.chats.docs.len()));
    }

    // ---- books ------------------------------------------------------------

    fn open_books(&mut self) {
        self.chats.level = Level::Books;
        self.chats.book_sel = 0;
        self.chats.local = books::local_books();
        self.info("Press L to log in to digi4school and see your books");
    }

    pub fn books_login(&mut self) {
        if self.chats.listing {
            return self.info("Already loading your books");
        }
        if self.cfg.digi4school_email.is_empty() {
            return self.open_prompt(Prompt::BooksEmail, "");
        }
        if self.chats.password.is_none() {
            self.chats.password = books::load_password(&self.cfg.digi4school_email);
            self.chats.password_saved = self.chats.password.is_some();
        }
        if self.chats.password.is_none() {
            return self.open_prompt(Prompt::BooksPassword, "");
        }
        self.chats.listing = true;
        self.info("Logging in to digi4school…");
        self.run_book_helper(vec!["list".to_string()]);
    }

    pub fn set_books_email(&mut self, email: &str) -> Result<()> {
        if !email.contains('@') {
            bail!("That doesn't look like an email address");
        }
        self.cfg.digi4school_email = email.to_string();
        config::save(&self.cfg)?;
        self.open_prompt(Prompt::BooksPassword, "");
        Ok(())
    }

    fn books_enter(&mut self) {
        let row = self.book_rows().get(self.chats.book_sel).map(|r| match r {
            BookRow::Local(p) => Ok((*p).clone()),
            BookRow::Remote(b) => Err(b.title.clone()),
        });
        match row {
            Some(Ok(path)) => self.want_editor = Some(EditTarget::Pdf(path)),
            Some(Err(title)) => self.download_book(title),
            None => self.books_login(),
        }
    }

    fn download_book(&mut self, title: String) {
        if self.chats.job.is_some() {
            return self.info("A book is already downloading. Press x to cancel it");
        }
        if self.chats.password.is_none() {
            return self.books_login();
        }
        let cancel = self.run_book_helper(vec!["download".to_string(), title.clone()]);
        self.chats.job = Some(BookJob { title, status: "starting…".into(), cancel });
    }

    fn cancel_book(&mut self) {
        if let Some(job) = &self.chats.job {
            job.cancel.store(true, Ordering::Relaxed);
            self.info("Stopping. Saved pages are kept, so downloading it again carries on");
        }
    }

    /// Runs the helper with the login on stdin, and forwards its reports to the app.
    fn run_book_helper(&mut self, args: Vec<String>) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        if cfg!(test) {
            return cancel;
        }
        let login = json!({"email": self.cfg.digi4school_email, "password": self.chats.password.clone().unwrap_or_default()});
        let (tx, flag) = (self.tx.clone(), cancel.clone());
        std::thread::spawn(move || {
            let finish = |message: String| {
                let _ = tx.send(Bg::Book(json!({"event": "error", "message": message})));
            };
            let Ok(exe) = std::env::current_exe() else { return finish("Couldn't find the program".into()) };
            let child = Command::new(exe).arg("books-helper").args(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => return finish(format!("Couldn't start the download: {e}")),
            };
            if let Some(mut stdin) = child.stdin.take() {
                let _ = writeln!(stdin, "{login}");
            }
            drop(login);
            let mut ended = false;
            if let Some(stdout) = child.stdout.take() {
                for line in std::io::BufReader::new(stdout).lines().map_while(std::result::Result::ok) {
                    if flag.load(Ordering::Relaxed) {
                        let _ = child.kill();
                        let _ = tx.send(Bg::Book(json!({"event": "cancelled"})));
                        ended = true;
                        break;
                    }
                    if let Ok(event) = serde_json::from_str::<Value>(&line) {
                        ended = ended || matches!(event["event"].as_str(), Some("error" | "done" | "books"));
                        let _ = tx.send(Bg::Book(event));
                    }
                }
            }
            let _ = child.wait();
            if !ended {
                finish("The download stopped unexpectedly".into());
            }
        });
        cancel
    }

    pub fn on_book_event(&mut self, event: Value) {
        match event["event"].as_str() {
            Some("books") => {
                self.chats.listing = false;
                self.chats.remote = serde_json::from_value(event["books"].clone()).unwrap_or_default();
                self.chats.local = books::local_books();
                if self.chats.remote.is_empty() {
                    self.error("digi4school listed no books that are still valid. Run lockin books-debug if that's wrong");
                } else {
                    self.info(format!("{} book(s) on your account. Enter downloads one", self.chats.remote.len()));
                }
                // A login that worked is the right moment to offer saving it.
                if !self.chats.password_saved && !self.chats.asked_to_save && self.mode == Mode::Normal {
                    self.chats.asked_to_save = true;
                    self.mode = Mode::Confirm(Confirm::SaveBooksPassword);
                }
            }
            Some("progress") => {
                if let Some(job) = &mut self.chats.job {
                    job.status = format!("page {}/{}", event["page"], event["total"]);
                }
            }
            Some("merging") => {
                if let Some(job) = &mut self.chats.job {
                    job.status = "joining pages…".into();
                }
            }
            Some("done") => {
                self.chats.job = None;
                self.chats.local = books::local_books();
                self.info(format!("Downloaded {}. Add it to a chat with f then a", event["title"].as_str().unwrap_or("the book")));
            }
            Some("cancelled") => {
                self.chats.job = None;
                self.info("Download stopped. Saved pages are kept");
            }
            _ => {
                let message = event["message"].as_str().unwrap_or("unknown error").to_string();
                // A wrong password shouldn't be retried silently on the next try.
                if message.starts_with("Login failed") {
                    self.chats.password = None;
                    if self.chats.password_saved {
                        books::forget_password(&self.cfg.digi4school_email);
                        self.chats.password_saved = false;
                    }
                }
                self.chats.listing = false;
                self.chats.job = None;
                self.error(message);
            }
        }
        self.clamp_chats();
    }

    pub fn save_books_password(&mut self) -> Result<()> {
        let Some(password) = self.chats.password.clone() else { return Ok(()) };
        let place = books::save_password(&self.cfg.digi4school_email, &password)?;
        self.chats.password_saved = true;
        self.info(format!("Password saved in {place}. Settings can forget it"));
        Ok(())
    }

    pub fn forget_books_password(&mut self) {
        books::forget_password(&self.cfg.digi4school_email);
        self.chats.password = None;
        self.chats.password_saved = false;
        self.chats.asked_to_save = false;
        self.info("Saved digi4school password removed");
    }

    pub fn delete_book(&mut self, path: &Path) -> Result<()> {
        std::fs::remove_file(path)?;
        if let Some(dir) = path.parent().filter(|d| *d != books::books_dir()) {
            std::fs::remove_dir(dir).ok(); // only removes the folder once it's empty
        }
        self.chats.local = books::local_books();
        self.clamp_chats();
        self.info("Book deleted from this computer");
        Ok(())
    }
}
