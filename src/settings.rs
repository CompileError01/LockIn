//! What the Settings tab lists, and how each row reads and changes the config.

use crate::config::{parse_times, Config};
use anyhow::{bail, Result};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Key {
    DailyEnabled,
    DailyTimes,
    DailyDays,
    DailyCatchUp,
    DailyClaude,
    SyncOnOpen,
    SyncInterval,
    RemindOn,
    RemindDeadlines,
    RemindStudy,
    RemindNudge,
    QuietHours,
    Desktop,
    Phone,
    PhoneReplies,
    NtfyServer,
    NtfyTopic,
    Model,
    TutorModel,
    TutorLanguage,
    AboutMe,
    StudyAuto,
    StudyLength,
    StudyPerDay,
    StudyUntil,
    StudyFreeFrom,
    StudyFreeUntil,
    PlanNow,
    FocusMinutes,
    BreakMinutes,
    Vault,
    NewNoteDir,
    Editor,
    Icons,
    RunDaily,
    SyncNow,
    TestNotify,
    ForgetBooksPassword,
    EditFile,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Toggle,
    Choice,
    Text,
    Action,
}

pub const ROWS: &[(&str, Key)] = &[
    ("Daily briefing", Key::DailyEnabled),
    ("Daily briefing", Key::DailyTimes),
    ("Daily briefing", Key::DailyDays),
    ("Daily briefing", Key::DailyCatchUp),
    ("Daily briefing", Key::DailyClaude),
    ("Moodle updates", Key::SyncOnOpen),
    ("Moodle updates", Key::SyncInterval),
    ("Reminders", Key::RemindOn),
    ("Reminders", Key::RemindDeadlines),
    ("Reminders", Key::RemindStudy),
    ("Reminders", Key::RemindNudge),
    ("Reminders", Key::QuietHours),
    ("Notifications", Key::Desktop),
    ("Notifications", Key::Phone),
    ("Notifications", Key::PhoneReplies),
    ("Notifications", Key::NtfyServer),
    ("Notifications", Key::NtfyTopic),
    ("Claude", Key::Model),
    ("Claude", Key::TutorModel),
    ("Claude", Key::TutorLanguage),
    ("Claude", Key::AboutMe),
    ("Study planner", Key::StudyAuto),
    ("Study planner", Key::StudyLength),
    ("Study planner", Key::StudyPerDay),
    ("Study planner", Key::StudyUntil),
    ("Study planner", Key::StudyFreeFrom),
    ("Study planner", Key::StudyFreeUntil),
    ("Study planner", Key::PlanNow),
    ("Focus timer", Key::FocusMinutes),
    ("Focus timer", Key::BreakMinutes),
    ("Notes", Key::Vault),
    ("Notes", Key::NewNoteDir),
    ("Notes", Key::Editor),
    ("Notes", Key::Icons),
    ("Actions", Key::RunDaily),
    ("Actions", Key::SyncNow),
    ("Actions", Key::TestNotify),
    ("Actions", Key::ForgetBooksPassword),
    ("Actions", Key::EditFile),
];

const SYNC_STEPS: [i64; 7] = [0, 15, 30, 60, 120, 240, 480];
const MODELS: [&str; 3] = ["haiku", "sonnet", "opus"];
const LANGUAGES: [&str; 3] = ["auto", "English", "Deutsch"];
const STUDY_LEADS: [i64; 5] = [0, 5, 10, 15, 30];
const STUDY_LENGTHS: [i64; 5] = [25, 45, 50, 60, 90];

pub fn kind(key: Key) -> Kind {
    use Key::*;
    match key {
        DailyEnabled | DailyCatchUp | DailyClaude | SyncOnOpen | RemindOn | Desktop | Phone | PhoneReplies | Icons | StudyAuto => Kind::Toggle,
        DailyDays | SyncInterval | Model | TutorModel | TutorLanguage | FocusMinutes | BreakMinutes | StudyLength | StudyPerDay | RemindStudy => Kind::Choice,
        DailyTimes | NtfyServer | NtfyTopic | AboutMe | Vault | NewNoteDir | Editor | StudyUntil | StudyFreeFrom | StudyFreeUntil | RemindDeadlines | RemindNudge | QuietHours => Kind::Text,
        RunDaily | SyncNow | TestNotify | ForgetBooksPassword | EditFile | PlanNow => Kind::Action,
    }
}

pub fn touches_timer(key: Key) -> bool {
    matches!(key, Key::DailyEnabled | Key::DailyTimes | Key::DailyDays | Key::DailyCatchUp | Key::RemindOn)
}

pub fn label(key: Key) -> &'static str {
    match key {
        Key::DailyEnabled => "Run automatically",
        Key::DailyTimes => "Times",
        Key::DailyDays => "Days",
        Key::DailyCatchUp => "Catch up missed runs",
        Key::DailyClaude => "Use Claude",
        Key::SyncOnOpen => "Sync when the app opens",
        Key::SyncInterval => "Sync while the app is open",
        Key::RemindOn => "Send reminders",
        Key::RemindDeadlines => "Warn before deadlines",
        Key::RemindStudy => "Before study sessions",
        Key::RemindNudge => "Study nudge and tests tomorrow",
        Key::QuietHours => "Quiet hours",
        Key::Desktop => "Desktop",
        Key::Phone => "Phone (ntfy)",
        Key::PhoneReplies => "Text LockIn from your phone",
        Key::NtfyServer => "ntfy server",
        Key::NtfyTopic => "ntfy topic",
        Key::Model => "Model",
        Key::TutorModel => "Chat model",
        Key::TutorLanguage => "Chat language",
        Key::AboutMe => "About you",
        Key::StudyAuto => "Plan new tests automatically",
        Key::StudyLength => "Session length",
        Key::StudyPerDay => "Most sessions a day",
        Key::StudyUntil => "Finish by, on school days",
        Key::StudyFreeFrom => "Start from, on free days",
        Key::StudyFreeUntil => "Finish by, on free days",
        Key::PlanNow => "Plan my study sessions now",
        Key::FocusMinutes => "Focus length",
        Key::BreakMinutes => "Break length",
        Key::Vault => "Vault folder",
        Key::NewNoteDir => "Folder for :note",
        Key::Editor => "Editor",
        Key::Icons => "File icons",
        Key::RunDaily => "Run the daily job now",
        Key::SyncNow => "Sync Moodle now",
        Key::TestNotify => "Send a test notification",
        Key::ForgetBooksPassword => "Forget digi4school password",
        Key::EditFile => "Open the config file",
    }
}

pub fn value(cfg: &Config, key: Key) -> String {
    let on_off = |b: bool| if b { "on" } else { "off" }.to_string();
    match key {
        Key::DailyEnabled => on_off(cfg.daily_enabled),
        Key::DailyTimes => cfg.daily_times.join(", "),
        Key::DailyDays => if cfg.daily_weekdays_only { "Mon to Fri" } else { "every day" }.into(),
        Key::DailyCatchUp => on_off(cfg.daily_catch_up),
        Key::DailyClaude => on_off(cfg.daily_use_claude),
        Key::SyncOnOpen => on_off(cfg.sync_on_open),
        Key::SyncInterval => match cfg.sync_interval_minutes {
            0 => "off".into(),
            m if m % 60 == 0 => format!("every {} h", m / 60),
            m => format!("every {m} min"),
        },
        Key::RemindOn => on_off(cfg.reminders_enabled),
        Key::RemindDeadlines if cfg.remind_deadline_hours.is_empty() => "off".into(),
        Key::RemindDeadlines => format!("{} h before", hours_text(cfg)),
        Key::RemindStudy => match cfg.remind_study_minutes {
            0 => "off".into(),
            m => format!("{m} min before"),
        },
        Key::RemindNudge if cfg.remind_nudge_time.is_empty() => "off".into(),
        Key::RemindNudge => cfg.remind_nudge_time.clone(),
        Key::QuietHours if cfg.quiet_hours.is_empty() => "off".into(),
        Key::QuietHours => cfg.quiet_hours.replace('-', " to "),
        Key::Desktop => on_off(cfg.desktop_notifications),
        Key::Phone => on_off(cfg.ntfy_enabled),
        Key::PhoneReplies => on_off(cfg.ntfy_replies),
        Key::NtfyServer => cfg.ntfy_server.clone(),
        Key::NtfyTopic => cfg.ntfy_topic.clone(),
        Key::Model => cfg.model.clone(),
        Key::TutorModel => cfg.tutor_model.clone(),
        Key::TutorLanguage => cfg.tutor_language.clone(),
        Key::AboutMe => cfg.about_me.clone(),
        Key::StudyAuto => on_off(cfg.study_auto_plan),
        Key::StudyLength => format!("{} min", cfg.study_session_minutes),
        Key::StudyPerDay => cfg.study_max_per_day.to_string(),
        Key::StudyUntil => cfg.study_until.clone(),
        Key::StudyFreeFrom => cfg.study_free_day_from.clone(),
        Key::StudyFreeUntil => cfg.study_free_day_until.clone(),
        Key::PlanNow => "press Enter".into(),
        Key::FocusMinutes => format!("{} min", cfg.focus_minutes),
        Key::BreakMinutes => format!("{} min", cfg.break_minutes),
        Key::Vault => cfg.vault.clone(),
        Key::NewNoteDir => cfg.new_note_dir.clone(),
        Key::Editor => cfg.editor.clone(),
        Key::Icons => on_off(cfg.nerd_icons),
        Key::RunDaily | Key::SyncNow | Key::TestNotify | Key::ForgetBooksPassword | Key::EditFile => "press Enter".into(),
    }
}

pub fn help(key: Key) -> &'static str {
    match key {
        Key::DailyEnabled => "Runs the daily job in the background, even when the app is closed: sync Moodle, ask Claude, then notify you.",
        Key::DailyTimes => "When the daily job runs. Use 24-hour times, and separate several with commas, like 07:00, 15:00. Each run uses some of your Claude usage.",
        Key::DailyDays => "Choose every day, or only Monday to Friday.",
        Key::DailyCatchUp => "If the computer was off or asleep at the scheduled time, run after your next login.",
        Key::DailyClaude => "When off, the daily job still syncs and notifies, but sends a plain list of what's due and no plan. This uses no Claude usage.",
        Key::SyncOnOpen => "Pull the latest deadlines each time you open LockIn.",
        Key::SyncInterval => "How often to pull from Moodle while LockIn stays open. This doesn't use Claude.",
        Key::RemindOn => "Between briefings, LockIn checks every 10 minutes for deadlines coming up, study sessions about to start and tests tomorrow. It goes to your desktop and phone, uses no Claude, and sends each reminder once.",
        Key::RemindDeadlines => "Hours before a deadline to warn you, like 24, 6, 2. If several have passed at once, you get only the closest. Type off for none.",
        Key::RemindStudy => "A heads-up before each study session from the Week tab.",
        Key::RemindNudge => "At this time, if you've done less than 20 minutes of focus today and something is due within a week, you get a nudge to study. Tests tomorrow are announced from this time too. Type off for neither.",
        Key::QuietHours => "No reminders between these times, like 22:00-07:00. Deadline warnings wait until it's over. Type off to allow them any time.",
        Key::Desktop => "Show notifications on this computer through mako.",
        Key::Phone => "Push notifications to your phone through ntfy. Task titles are sent to the ntfy server.",
        Key::PhoneReplies => "Send a message to your ntfy topic to add tasks and events, tick things off, sync, or ask Claude. It works while the app is closed, if you're logged in. Anyone who knows the topic can do the same, so the first time this goes on you get a new private topic. Run lockin phone to scan it into your phone. It can't delete anything, submit to Moodle or read your files. Pressing Enter here switches it off when it's on.",
        Key::NtfyServer => "The server that delivers phone notifications. Change it only if you host your own.",
        Key::NtfyTopic => "Subscribe to this name in the ntfy app. Anyone who knows it can read your notifications, so keep it private.",
        Key::Model => "The Claude model for the briefing, study plans and phone replies. Haiku uses the least of your usage, and Opus uses the most.",
        Key::TutorModel => "The Claude model for the Chats tab. Opus handles harder math better, and uses more of your usage.",
        Key::TutorLanguage => "auto replies in the language you write in, and starts lessons in the material's language. Or always use English or Deutsch.",
        Key::AboutMe => "A short line Claude reads when planning, like your school and year. Leave it empty if you prefer.",
        Key::StudyAuto => "The daily job plans study sessions when a new test appears. It plans each test once, so a session you delete stays deleted.",
        Key::StudyLength => "How long each study session lasts, with a 15-minute break between sessions.",
        Key::StudyPerDay => "The most study sessions on any one day, across all your tests.",
        Key::StudyUntil => "On school days, sessions start 30 minutes after your last lesson and end by this time.",
        Key::StudyFreeFrom => "Days without lessons, like weekends and holidays, start at this time.",
        Key::StudyFreeUntil => "Days without lessons end by this time.",
        Key::PlanNow => "Replaces planned sessions that are still ahead, and keeps any you've edited or moved. Press u in the app to undo it.",
        Key::FocusMinutes => "Default length of a focus session.",
        Key::BreakMinutes => "Default length of a break.",
        Key::Vault => "The folder shown in the Notes tab. Its contents are never sent to Claude.",
        Key::NewNoteDir => "Where :note puts new notes, inside the vault. Press a in the Notes tab to create in the current folder instead.",
        Key::Editor => "The command that opens notes, like nvim.",
        Key::Icons => "File-type icons in the Notes tab. Ghostty includes them. Turn this off if you see empty boxes in another terminal.",
        Key::RunDaily => "Syncs, asks Claude and sends the notifications right now, exactly like a scheduled run.",
        Key::SyncNow => "Pulls from Moodle now, without using Claude.",
        Key::TestNotify => "Sends a test to every notification channel that's on.",
        Key::ForgetBooksPassword => "Removes your saved digi4school password. LockIn asks for it again next time.",
        Key::EditFile => "Opens config.toml in your editor. Changes apply when you close it. If you change times there, run lockin install afterwards.",
    }
}

/// Flips a toggle or steps a choice. `dir` is +1 or -1.
pub fn step(cfg: &mut Config, key: Key, dir: i64) {
    match key {
        Key::DailyEnabled => cfg.daily_enabled = !cfg.daily_enabled,
        Key::DailyCatchUp => cfg.daily_catch_up = !cfg.daily_catch_up,
        Key::DailyClaude => cfg.daily_use_claude = !cfg.daily_use_claude,
        Key::DailyDays => cfg.daily_weekdays_only = !cfg.daily_weekdays_only,
        Key::SyncOnOpen => cfg.sync_on_open = !cfg.sync_on_open,
        Key::RemindOn => cfg.reminders_enabled = !cfg.reminders_enabled,
        Key::RemindStudy => {
            let i = STUDY_LEADS.iter().position(|m| *m == cfg.remind_study_minutes).unwrap_or(2) as i64;
            cfg.remind_study_minutes = STUDY_LEADS[(i + dir).rem_euclid(STUDY_LEADS.len() as i64) as usize];
        }
        Key::Desktop => cfg.desktop_notifications = !cfg.desktop_notifications,
        Key::Phone => cfg.ntfy_enabled = !cfg.ntfy_enabled,
        Key::PhoneReplies => cfg.ntfy_replies = !cfg.ntfy_replies,
        Key::Icons => cfg.nerd_icons = !cfg.nerd_icons,
        Key::SyncInterval => {
            let i = SYNC_STEPS.iter().position(|m| *m == cfg.sync_interval_minutes).unwrap_or(3) as i64;
            cfg.sync_interval_minutes = SYNC_STEPS[(i + dir).rem_euclid(SYNC_STEPS.len() as i64) as usize];
        }
        Key::Model => {
            let i = MODELS.iter().position(|m| *m == cfg.model).unwrap_or(1) as i64;
            cfg.model = MODELS[(i + dir).rem_euclid(MODELS.len() as i64) as usize].into();
        }
        Key::TutorLanguage => {
            let i = LANGUAGES.iter().position(|l| *l == cfg.tutor_language).unwrap_or(0) as i64;
            cfg.tutor_language = LANGUAGES[(i + dir).rem_euclid(LANGUAGES.len() as i64) as usize].into();
        }
        Key::TutorModel => {
            let i = MODELS.iter().position(|m| *m == cfg.tutor_model).unwrap_or(1) as i64;
            cfg.tutor_model = MODELS[(i + dir).rem_euclid(MODELS.len() as i64) as usize].into();
        }
        Key::StudyAuto => cfg.study_auto_plan = !cfg.study_auto_plan,
        Key::StudyLength => {
            let i = STUDY_LENGTHS.iter().position(|m| *m == cfg.study_session_minutes).unwrap_or(2) as i64;
            cfg.study_session_minutes = STUDY_LENGTHS[(i + dir).rem_euclid(STUDY_LENGTHS.len() as i64) as usize];
        }
        Key::StudyPerDay => cfg.study_max_per_day = (cfg.study_max_per_day + dir).clamp(1, 6),
        Key::FocusMinutes => cfg.focus_minutes = (cfg.focus_minutes + 5 * dir).clamp(5, 180),
        Key::BreakMinutes => cfg.break_minutes = (cfg.break_minutes + dir).clamp(1, 60),
        _ => {}
    }
}

fn hours_text(cfg: &Config) -> String {
    cfg.remind_deadline_hours.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")
}

pub fn text_value(cfg: &Config, key: Key) -> String {
    match key {
        Key::RemindDeadlines => hours_text(cfg),
        Key::RemindNudge => cfg.remind_nudge_time.clone(),
        Key::QuietHours => cfg.quiet_hours.clone(),
        _ => value(cfg, key),
    }
}

pub fn set_text(cfg: &mut Config, key: Key, text: &str) -> Result<()> {
    let text = text.trim();
    match key {
        Key::DailyTimes => cfg.daily_times = parse_times(text)?,
        Key::NtfyServer => {
            if !text.starts_with("https://") && !text.starts_with("http://") {
                bail!("The server must start with https:// or http://");
            }
            cfg.ntfy_server = text.trim_end_matches('/').to_string();
        }
        Key::NtfyTopic => {
            let valid = !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !valid {
                bail!("A topic can only use letters, numbers, - and _");
            }
            crate::config::change_topic(cfg, text.to_string());
        }
        Key::Vault => {
            let expanded = match text.strip_prefix("~/") {
                Some(rest) => crate::config::home().join(rest),
                None => text.into(),
            };
            if !expanded.is_absolute() || !expanded.is_dir() {
                bail!("{} isn't a folder", expanded.display());
            }
            cfg.vault = expanded.display().to_string();
        }
        Key::NewNoteDir => {
            if text.is_empty() || text.starts_with('/') || text.split('/').any(|p| p == "..") {
                bail!("Use a folder inside the vault, like LockIn");
            }
            cfg.new_note_dir = text.to_string();
        }
        Key::StudyUntil | Key::StudyFreeFrom | Key::StudyFreeUntil => {
            let mut times = parse_times(text)?;
            if times.len() != 1 {
                bail!("Enter one time, like 20:00");
            }
            let time = times.remove(0);
            match key {
                Key::StudyUntil => cfg.study_until = time,
                Key::StudyFreeFrom => cfg.study_free_day_from = time,
                _ => cfg.study_free_day_until = time,
            }
        }
        Key::RemindDeadlines => {
            let mut hours = Vec::new();
            if !text.eq_ignore_ascii_case("off") {
                for tok in text.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()) {
                    match tok.trim_end_matches('h').parse::<i64>() {
                        Ok(h) if (1..=336).contains(&h) => hours.push(h),
                        _ => bail!("'{tok}' isn't a number of hours. Use something like 24, 6, 2"),
                    }
                }
            }
            hours.sort_unstable_by(|a, b| b.cmp(a));
            hours.dedup();
            cfg.remind_deadline_hours = hours;
        }
        Key::RemindNudge if text.is_empty() || text.eq_ignore_ascii_case("off") => cfg.remind_nudge_time.clear(),
        Key::RemindNudge => {
            let times = parse_times(text)?;
            if times.len() != 1 {
                bail!("Enter one time, like 17:00, or off");
            }
            cfg.remind_nudge_time = times[0].clone();
        }
        Key::QuietHours if text.is_empty() || text.eq_ignore_ascii_case("off") => cfg.quiet_hours.clear(),
        Key::QuietHours => {
            let tidy = text.replace(" to ", "-").replace(' ', "");
            let Some((a, b)) = tidy.split_once('-') else { bail!("Use two times, like 22:00-07:00, or off") };
            let (a, b) = (parse_times(a)?, parse_times(b)?);
            if a.len() != 1 || b.len() != 1 || a == b {
                bail!("Use two different times, like 22:00-07:00, or off");
            }
            cfg.quiet_hours = format!("{}-{}", a[0], b[0]);
        }
        Key::AboutMe => cfg.about_me = text.to_string(),
        Key::Editor => {
            if text.is_empty() {
                bail!("Enter an editor command, like nvim");
            }
            cfg.editor = text.to_string();
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_has_a_label_value_and_help() {
        let cfg = Config::default();
        for (_, key) in ROWS {
            assert!(!label(*key).is_empty());
            assert!(!help(*key).is_empty());
            if kind(*key) != Kind::Text {
                assert!(!value(&cfg, *key).is_empty(), "{key:?}");
            }
        }
    }

    #[test]
    fn stepping_wraps_and_stays_in_range() {
        let mut cfg = Config::default();
        cfg.sync_interval_minutes = 480;
        step(&mut cfg, Key::SyncInterval, 1);
        assert_eq!(cfg.sync_interval_minutes, 0);
        assert_eq!(value(&cfg, Key::SyncInterval), "off");
        step(&mut cfg, Key::SyncInterval, -1);
        assert_eq!(value(&cfg, Key::SyncInterval), "every 8 h");
        cfg.focus_minutes = 5;
        step(&mut cfg, Key::FocusMinutes, -1);
        assert_eq!(cfg.focus_minutes, 5);
        step(&mut cfg, Key::Model, 1);
        assert_eq!(cfg.model, "opus");
    }

    #[test]
    fn bad_text_is_rejected_and_leaves_config_alone() {
        let mut cfg = Config::default();
        assert!(set_text(&mut cfg, Key::DailyTimes, "noon").is_err());
        assert_eq!(cfg.daily_times, vec!["15:00"]);
        assert!(set_text(&mut cfg, Key::NtfyServer, "ntfy.sh").is_err());
        assert!(set_text(&mut cfg, Key::NtfyTopic, "has space").is_err());
        assert!(set_text(&mut cfg, Key::Vault, "/definitely/not/here").is_err());
        assert!(set_text(&mut cfg, Key::NewNoteDir, "../out").is_err());
        assert!(set_text(&mut cfg, Key::RemindDeadlines, "24, soon").is_err());
        assert!(set_text(&mut cfg, Key::QuietHours, "22:00").is_err());
        assert_eq!(cfg.remind_deadline_hours, vec![24, 6, 2]);
        set_text(&mut cfg, Key::RemindDeadlines, "2h 48, 6 6").unwrap();
        assert_eq!(cfg.remind_deadline_hours, vec![48, 6, 2]);
        set_text(&mut cfg, Key::QuietHours, "23:00 to 6:30").unwrap();
        assert_eq!(cfg.quiet_hours, "23:00-06:30");
        set_text(&mut cfg, Key::RemindNudge, "off").unwrap();
        assert_eq!(value(&cfg, Key::RemindNudge), "off");
        set_text(&mut cfg, Key::DailyTimes, "16:30, 7:00").unwrap();
        assert_eq!(cfg.daily_times, vec!["07:00", "16:30"]);
    }
}
