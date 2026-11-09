mod app;
mod config;
mod db;
mod files;
mod form;
mod pdf;
mod services;
mod settings;
mod timeutil;
mod ui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "lockin", version, about = "Neovim-style organizer with Moodle sync and a daily Claude briefing")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Pull deadlines, calendar events and grades from Moodle
    Sync,
    /// Ask Claude for a new briefing and print it
    Brief,
    /// Sync, brief and notify: what the daily timer runs
    Daily,
    /// Print what's open
    Today,
    /// Add a task, e.g. lockin add "Read chapter 4 @fri !! #AM"
    Add { text: Vec<String> },
    /// Read a PDF in the terminal
    Pdf { file: std::path::PathBuf },
    /// Open a file: PDFs in the terminal, anything else in your usual app
    Open { file: std::path::PathBuf },
    /// Send a test notification to the desktop and phone
    Notify,
    /// Install or update the daily systemd timer from your settings
    Install,
    /// Show config, timer and notification details
    Status,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = config::load()?;
    let open_db = || db::Db::open(&config::db_path());
    match cli.cmd {
        None => run_tui(cfg),
        Some(Cmd::Sync) => {
            let s = services::sync(&mut open_db()?)?;
            println!("{} new, {} updated, {} finished", s.added, s.updated, s.completed);
            Ok(())
        }
        Some(Cmd::Brief) => {
            let db = open_db()?;
            let out = services::brief(&db, &cfg)?;
            println!("{}\n\n{}", out.headline, out.plan);
            Ok(())
        }
        Some(Cmd::Daily) => services::daily(&mut open_db()?, &cfg),
        Some(Cmd::Today) => {
            let db = open_db()?;
            let tasks = db.tasks()?;
            let open: Vec<_> = tasks.iter().filter(|t| !t.done).collect();
            if open.is_empty() {
                println!("Nothing open.");
            }
            for t in open {
                let due = t.due.map_or("no date".to_string(), |d| format!("{} ({})", timeutil::fmt_abs(d), timeutil::fmt_rel(d)));
                println!("{:<24} {:<8} {}", due, ui::course_tag(&t.course), t.title);
            }
            Ok(())
        }
        Some(Cmd::Add { text }) => {
            let q = timeutil::parse_quick(&text.join(" "));
            anyhow::ensure!(!q.title.is_empty(), "a task needs a title");
            let eod = chrono::NaiveTime::from_hms_opt(23, 59, 0).context("bad time")?;
            let due = match &q.when {
                Some(w) => Some(timeutil::parse_when(w, eod, chrono::Local::now()).with_context(|| format!("couldn't read the date '{w}'"))?),
                None => None,
            };
            open_db()?.add_task(&q.title, due, q.priority, q.course.as_deref().unwrap_or(""), q.minutes)?;
            println!("Added: {}", q.title);
            Ok(())
        }
        Some(Cmd::Pdf { file }) => pdf::view(&file),
        Some(Cmd::Open { file }) => pdf::open_any(&file),
        Some(Cmd::Notify) => {
            let errors = services::notify_all(&cfg, "LockIn test", "Notifications are working.", false, false);
            for e in &errors {
                eprintln!("{e}");
            }
            anyhow::ensure!(errors.is_empty(), "{} channel(s) failed", errors.len());
            println!("Sent.");
            Ok(())
        }
        Some(Cmd::Install) => {
            services::apply_timer(&cfg)?;
            println!("Daily timer updated. Next run: {}", services::timer_next_run());
            Ok(())
        }
        Some(Cmd::Status) => {
            let db = open_db()?;
            let fmt = |k: &str| db.meta(k).and_then(|v| v.parse().ok()).map_or("never".to_string(), |t: i64| timeutil::fmt_abs(t));
            println!("config      {}", config::config_path().display());
            println!("database    {}", config::db_path().display());
            println!("vault       {}", cfg.vault);
            println!("phone topic {}/{}", cfg.ntfy_server, cfg.ntfy_topic);
            println!("last sync   {}", fmt("last_sync"));
            println!("last daily  {}", fmt("last_daily"));
            Ok(())
        }
    }
}

fn run_tui(cfg: config::Config) -> Result<()> {
    let mut app = app::App::new(cfg)?;
    app.startup();
    let mut term = ratatui::init();
    let result = (|| -> Result<()> {
        loop {
            app.prepare();
            term.draw(|f| ui::draw(f, &app))?;
            if event::poll(Duration::from_millis(250))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        app.on_key(key);
                    }
                }
            }
            app.tick();
            if let Some(target) = app.want_editor.take() {
                ratatui::restore();
                let opened = app::run_external(&app.cfg, &target);
                // A new terminal repaints every cell, because the background fills the screen.
                term = ratatui::init();
                if let Err(e) = opened {
                    app.error(e.to_string());
                }
                app.after_editor(target);
            }
            if app.quit {
                return Ok(());
            }
        }
    })();
    ratatui::restore();
    result
}
