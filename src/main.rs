mod app;
mod books;
mod chats;
mod config;
mod db;
mod files;
mod form;
mod pdf;
mod planner;
mod reminders;
mod replies;
mod services;
mod settings;
mod timeutil;
mod tutor;
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
    /// Pull deadlines and events from Moodle, and your timetable and tests from Untis
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
    /// Ask a teaching chat about one file, from the shell
    Teach { file: std::path::PathBuf, question: Vec<String> },
    /// Internal: talks to digi4school for the Chats tab, with the login on stdin
    #[command(hide = true)]
    BooksHelper { mode: String, title: Option<String> },
    /// Describe digi4school's responses with no personal data, for fixing problems
    BooksDebug,
    /// Turn texting LockIn from your phone on or off
    Replies { state: String },
    /// Show a QR code that connects your phone to your private topic
    Phone,
    /// Internal: the background service for phone replies
    #[command(hide = true)]
    Listen,
    /// Plan study sessions for your upcoming tests
    Plan,
    /// Send reminders that are due now: what the 10-minute timer runs
    Remind {
        /// Only print what would be sent
        #[arg(long)]
        preview: bool,
    },
    /// Send a test notification to the desktop and phone
    Notify,
    /// Install or update the daily systemd timer from your settings
    Install,
    /// Show config, timer and notification details
    Status,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(Cmd::BooksHelper { mode, title }) = &cli.cmd {
        return books::helper(mode, title.as_deref());
    }
    let cfg = config::load()?;
    let open_db = || db::Db::open(&config::db_path());
    match cli.cmd {
        None => run_tui(cfg),
        Some(Cmd::Sync) => {
            let mut db = open_db()?;
            let s = services::sync(&mut db)?;
            println!("{} new, {} updated, {} finished", s.added, s.updated, s.completed);
            if let Some(summary) = services::sync_untis(&mut db)? {
                println!("timetable: {summary}");
            }
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
        Some(Cmd::Teach { file, question }) => {
            let db = open_db()?;
            let path = file.canonicalize().with_context(|| format!("Can't open {}", file.display()))?;
            let rel = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let material = tutor::Material::File { path, rel };
            let id = match tutor::find_session(&db, &material.source_key())? {
                Some(s) => s.id,
                None => tutor::create_session(&db, "", &material)?,
            };
            let question = if question.is_empty() { tutor::OPENER.to_string() } else { question.join(" ") };
            tutor::add_message(&db, id, tutor::Role::You, &question)?;
            tutor::teach(&db, &cfg, id, &question)?;
            let all = tutor::messages(&db, id)?;
            let from = all.iter().rposition(|(role, _)| *role == tutor::Role::You).map_or(0, |i| i + 1);
            for (_, text) in &all[from..] {
                println!("{text}\n");
            }
            Ok(())
        }
        Some(Cmd::BooksHelper { .. }) => unreachable!("handled above"),
        Some(Cmd::BooksDebug) => books::debug(&cfg.digi4school_email),
        Some(Cmd::Plan) => {
            let mut db = open_db()?;
            if let Err(e) = services::sync_untis(&mut db) {
                eprintln!("{e}. Using the last synced timetable.");
            }
            let outcome = planner::plan(&db, &cfg, planner::Mode::Replan, true)?;
            for e in &outcome.added {
                println!("{}  {}  {}", timeutil::fmt_abs(e.start), e.title, e.notes);
            }
            println!("{}", outcome.summary());
            Ok(())
        }
        Some(Cmd::Listen) => replies::listen(&cfg),
        Some(Cmd::Phone) => phone(&cfg),
        Some(Cmd::Remind { preview }) => reminders::run(&cfg, preview),
        Some(Cmd::Replies { state }) => {
            let mut cfg = cfg;
            let on = match state.as_str() {
                "on" => true,
                "off" => false,
                _ => anyhow::bail!("Use: lockin replies on, or lockin replies off"),
            };
            let changed = services::set_replies(&mut cfg, on)?;
            println!("Phone replies are {}.", if on { "on" } else { "off" });
            if changed {
                println!("You have a new private topic. Run lockin phone to scan it into your phone.");
            }
            Ok(())
        }
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
            services::apply_listener(&cfg)?;
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
            println!("replies     {} (listener: {})", if cfg.ntfy_replies { "on" } else { "off" }, services::listener_state());
            println!("last sync   {}", fmt("last_sync"));
            println!("last daily  {}", fmt("last_daily"));
            println!("reminders   {} (last check: {})", if cfg.reminders_enabled { "on" } else { "off" }, fmt("last_remind"));
            if cfg.ntfy_replies {
                println!("last text   {}", fmt("ntfy_last_in"));
            }
            Ok(())
        }
    }
}

/// The link the ntfy Android app opens straight into "subscribe", so nothing has to be typed.
fn app_link(cfg: &config::Config) -> String {
    let server = cfg.ntfy_server.trim_end_matches('/');
    match server.strip_prefix("http://") {
        Some(host) => format!("ntfy://{host}/{}?secure=false", cfg.ntfy_topic),
        None => format!("ntfy://{}/{}", server.trim_start_matches("https://"), cfg.ntfy_topic),
    }
}

/// `lockin phone`: connect the phone, then wait for a text to prove it reaches LockIn.
fn phone(cfg: &config::Config) -> Result<()> {
    let web = format!("{}/{}", cfg.ntfy_server.trim_end_matches('/'), cfg.ntfy_topic);
    let tail: String = cfg.ntfy_topic.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
    println!("{}", services::qr_text(&app_link(cfg))?);
    println!("1. Scan this with your phone's camera. It opens the ntfy app, ready to subscribe. Tap Subscribe.");
    println!("   No app link? Open {web} in the phone's browser instead.");
    println!("2. In the ntfy app, open the subscription whose name ends in …{tail}.");
    println!("   If you have older LockIn subscriptions there, delete them: those topics aren't read any more.");
    println!("Keep the code private: anyone with it can read your notifications and send commands.");
    services::ntfy(cfg, "LockIn", &format!("This is the right topic (…{tail}). Write here to talk to LockIn. Try: hi"), false)?;
    if !cfg.ntfy_replies {
        println!("\nA test notification was sent. Phone replies are off, so this only gets notifications. Turn them on with: lockin replies on");
        return Ok(());
    }
    if services::listener_state() != "active" {
        println!("\n⚠ The background listener isn't running ({}). Try: lockin install", services::listener_state());
        return Ok(());
    }
    let db = db::Db::open(&config::db_path())?;
    let before = db.meta("ntfy_last_in");
    println!("\n3. Now type hi in that subscription and send it (the arrow at the bottom). Waiting up to 3 minutes, Ctrl+C to stop…");
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(180) {
        std::thread::sleep(Duration::from_secs(2));
        if db.meta("ntfy_last_in") != before {
            println!("✓ Your phone is connected. LockIn got the message and answered it.");
            return Ok(());
        }
    }
    println!("✗ Nothing arrived from your phone.");
    println!("  • Check the subscription's name ends in …{tail}. If not, delete it and scan the code again.");
    println!("  • In the app's settings, the default server should be {}.", cfg.ntfy_server);
    println!("  • Or send from the browser: open {web} and use the box at the bottom.");
    Ok(())
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
