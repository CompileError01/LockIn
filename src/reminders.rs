//! Reminders between the daily briefings: deadlines coming up, study sessions about to start,
//! tests tomorrow, and a nudge on days you haven't studied. A systemd timer runs `lockin remind`
//! every 10 minutes. Each reminder is sent once, and none are sent during quiet hours.
//!
//! This uses no Claude. Everything here is worked out from the database.

use crate::config::{db_path, Config};
use crate::db::{Db, Event, Task};
use crate::planner::SOURCE_UNTIS;
use crate::services::{self, Push};
use crate::timeutil::{fmt_abs, local, ts_of};
use anyhow::Result;
use chrono::{DateTime, Duration, Local, NaiveTime};
use std::collections::HashMap;

/// A reminder that was sent is forgotten after this, so the list can't grow forever.
const KEEP_SENT_DAYS: i64 = 30;
/// A study session that started longer ago than this is too late to remind about.
const STUDY_GRACE_SECS: i64 = 5 * 60;
/// The nudge isn't sent this long after its time, like when the computer comes on late at night.
const NUDGE_WINDOW_SECS: i64 = 3 * 3600;
/// Less focus time than this today counts as not having studied.
const STUDIED_MINUTES: i64 = 20;

#[derive(Debug, Clone)]
pub struct Reminder {
    /// Remembered once sent, so the same reminder never comes twice.
    pub key: String,
    /// More keys to remember with it, like the further-out deadline warnings it replaces.
    pub also: Vec<String>,
    pub push: Push,
    /// Moodle tasks may already be submitted, so these are checked against a fresh sync first.
    pub moodle: bool,
}

fn parse_hm(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

/// `22:00-07:00` style, which can wrap past midnight. Anything unreadable means no quiet hours.
pub fn parse_quiet(s: &str) -> Option<(NaiveTime, NaiveTime)> {
    let (a, b) = s.split_once('-')?;
    Some((parse_hm(a)?, parse_hm(b)?))
}

pub fn is_quiet(cfg: &Config, now: DateTime<Local>) -> bool {
    let Some((from, until)) = parse_quiet(&cfg.quiet_hours) else { return false };
    let t = now.time();
    if from <= until {
        t >= from && t < until
    } else {
        t >= from || t < until
    }
}

/// "5 h", "40 min" or "3 days", for how long is left.
pub fn left(secs: i64) -> String {
    let mins = (secs / 60).max(1);
    if mins < 120 {
        format!("{mins} min")
    } else if mins < 48 * 60 {
        format!("{} h", (mins + 30) / 60)
    } else {
        format!("{} days", (mins + 12 * 60) / (24 * 60))
    }
}

fn task_line(t: &Task) -> String {
    let course = if t.course.is_empty() { String::new() } else { format!(" ({})", crate::ui::course_tag(&t.course)) };
    format!("{}{course}", t.title)
}

fn task_buttons(t: &Task) -> Vec<(String, String)> {
    vec![("Done ✓".into(), format!("done {}", t.id)), ("What's due".into(), "list".into())]
}

fn deadline_reminders(cfg: &Config, now: i64, tasks: &[Task]) -> Vec<Reminder> {
    let mut hours: Vec<i64> = cfg.remind_deadline_hours.iter().copied().filter(|h| *h > 0).collect();
    hours.sort_unstable();
    hours.dedup();
    let mut out = Vec::new();
    for t in tasks.iter().filter(|t| !t.done) {
        let Some(due) = t.due.filter(|d| *d > now) else { continue };
        let crossed: Vec<i64> = hours.iter().copied().filter(|h| now >= due - h * 3600).collect();
        if crossed.is_empty() {
            continue;
        }
        let keys: Vec<String> = crossed.iter().map(|h| format!("due:{}:{due}:{h}", t.id)).collect();
        let secs = due - now;
        let mut body = format!("{} is due {}.", task_line(t), fmt_abs(due));
        if let Some(m) = t.effort_min.filter(|m| *m > 0) {
            body += &format!(" It takes about {m} min.");
        }
        if !t.ai_note.trim().is_empty() {
            body += &format!("\n{}", t.ai_note.trim());
        }
        if t.is_moodle() {
            body += "\nTick it off here only after you've submitted it on Moodle.";
        }
        let push = Push {
            title: format!("Due in {}: {}", left(secs), t.title),
            body,
            urgent: secs <= 6 * 3600,
            icon: if secs <= 6 * 3600 { "rotating_light".into() } else { "alarm_clock".into() },
            buttons: task_buttons(t),
            click: t.url.clone(),
        };
        out.push(Reminder { key: keys[0].clone(), also: keys[1..].to_vec(), push, moodle: t.is_moodle() });
    }
    out
}

fn study_reminders(cfg: &Config, now: i64, events: &[Event]) -> Vec<Reminder> {
    if cfg.remind_study_minutes <= 0 {
        return Vec::new();
    }
    let lead = cfg.remind_study_minutes * 60;
    events
        .iter()
        .filter(|e| e.is_study() && e.start <= now + lead && e.start > now - STUDY_GRACE_SECS)
        .map(|e| {
            let when = local(e.start).format("%H:%M");
            let mut body = format!("{} until {}.", e.title, local(e.end).format("%H:%M"));
            if !e.notes.trim().is_empty() {
                body += &format!("\n{}", e.notes.trim());
            }
            let title = if e.start > now { format!("Study session at {when}, in {}", left(e.start - now)) } else { format!("Study session now ({when})") };
            Reminder {
                key: format!("study:{}:{}", e.id, e.start),
                also: Vec::new(),
                push: Push { title, body, icon: "books".into(), buttons: vec![("What's due".into(), "list".into())], ..Default::default() },
                moodle: false,
            }
        })
        .collect()
}

/// The two once-a-day messages that go at the nudge time: tests tomorrow, and a nudge to study.
fn daily_reminders(cfg: &Config, now: DateTime<Local>, tasks: &[Task], events: &[Event], focus_today: i64) -> Vec<Reminder> {
    let Some(at) = parse_hm(&cfg.remind_nudge_time) else { return Vec::new() };
    let today = now.date_naive();
    let from = ts_of(today, at);
    let ts = now.timestamp();
    if ts < from {
        return Vec::new();
    }
    let mut out = Vec::new();
    let tomorrow = today + Duration::days(1);
    // Tests are sent any time after the nudge time, because missing one matters more.
    for e in events.iter().filter(|e| e.source == SOURCE_UNTIS && local(e.start).date_naive() == tomorrow) {
        let id = e.ext_id.clone().unwrap_or_else(|| e.id.to_string());
        let mut body = format!("{} at {}.", e.title, local(e.start).format("%H:%M"));
        if !e.course.is_empty() {
            body = format!("{}: {body}", crate::ui::course_tag(&e.course));
        }
        let sessions = events.iter().filter(|s| s.is_study() && s.start < e.start && s.start > ts - 14 * 86400 && s.title.contains(e.title.as_str())).count();
        if sessions > 0 {
            body += &format!(" You planned {sessions} study session{} for it.", if sessions == 1 { "" } else { "s" });
        }
        body += " Get some sleep tonight.";
        out.push(Reminder {
            key: format!("exam:{id}:{}", e.start),
            also: Vec::new(),
            push: Push { title: format!("Test tomorrow: {}", e.title), body, urgent: true, icon: "memo".into(), ..Default::default() },
            moodle: false,
        });
    }
    if ts >= from + NUDGE_WINDOW_SECS || focus_today >= STUDIED_MINUTES {
        return out;
    }
    // A session later today gets its own reminder.
    let end_of_day = ts_of(tomorrow, NaiveTime::MIN);
    if events.iter().any(|e| e.is_study() && e.start > ts && e.start < end_of_day) {
        return out;
    }
    let mut soon: Vec<&Task> = tasks.iter().filter(|t| !t.done && t.due.is_some_and(|d| d > ts && d < ts + 7 * 86400)).collect();
    soon.sort_by_key(|t| t.due);
    let next_exam = events.iter().filter(|e| e.source == SOURCE_UNTIS && e.start > ts && e.start < ts + 7 * 86400).min_by_key(|e| e.start);
    if soon.is_empty() && next_exam.is_none() {
        return out;
    }
    let mut lines = vec!["You haven't studied today. Up next:".to_string()];
    if let Some(e) = next_exam {
        lines.push(format!("• Test: {} in {}", e.title, left(e.start - ts)));
    }
    lines.extend(soon.iter().take(3).map(|t| format!("• {} in {}", task_line(t), left(t.due.unwrap_or(ts) - ts))));
    lines.push(format!("Even one {}-minute session helps.", cfg.focus_minutes));
    let first = soon.first().copied();
    out.push(Reminder {
        key: format!("nudge:{today}"),
        also: Vec::new(),
        push: Push {
            title: "Time to study".into(),
            body: lines.join("\n"),
            icon: "muscle".into(),
            buttons: first.map(task_buttons).unwrap_or_else(|| vec![("What's due".into(), "list".into())]),
            click: first.map(|t| t.url.clone()).unwrap_or_default(),
            ..Default::default()
        },
        moodle: false,
    });
    out
}

/// Everything that's due to be sent now and hasn't been sent before.
pub fn collect(cfg: &Config, now: DateTime<Local>, tasks: &[Task], events: &[Event], focus_today: i64, sent: &HashMap<String, i64>) -> Vec<Reminder> {
    if !cfg.reminders_enabled || is_quiet(cfg, now) {
        return Vec::new();
    }
    let ts = now.timestamp();
    let mut all = deadline_reminders(cfg, ts, tasks);
    all.extend(study_reminders(cfg, ts, events));
    all.extend(daily_reminders(cfg, now, tasks, events, focus_today));
    all.retain(|r| !sent.contains_key(&r.key));
    all
}

fn load_sent(db: &Db) -> HashMap<String, i64> {
    db.meta("reminders_sent").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// `lockin remind`: what the 10-minute timer runs. With `preview`, it only prints what it would send.
pub fn run(cfg: &Config, preview: bool) -> Result<()> {
    let mut db = Db::open(&db_path())?;
    let mut sent = load_sent(&db);
    let gather = |db: &Db, sent: &HashMap<String, i64>| -> Result<Vec<Reminder>> {
        Ok(collect(cfg, Local::now(), &db.tasks()?, &db.events()?, db.focus_minutes_today()?, sent))
    };
    let mut due = gather(&db, &sent)?;
    // Don't warn about homework that was handed in since the last sync.
    let last_sync: i64 = db.meta("last_sync").and_then(|v| v.parse().ok()).unwrap_or(0);
    if !preview && due.iter().any(|r| r.moodle) && crate::timeutil::now_ts() - last_sync > 3600 {
        match services::sync(&mut db) {
            Ok(_) => due = gather(&db, &sent)?,
            Err(e) => eprintln!("lockin remind: Moodle sync failed, sending anyway: {e}"),
        }
    }
    let now = crate::timeutil::now_ts();
    for r in &due {
        if preview {
            println!("{}\n{}\n", r.push.title, r.push.body);
            continue;
        }
        let errors = services::send_push(cfg, &r.push, false);
        let channels = usize::from(cfg.ntfy_enabled) + usize::from(cfg.desktop_notifications);
        for e in &errors {
            eprintln!("lockin remind: {e}");
        }
        // Tried again next time only if nothing got through.
        if channels > 0 && errors.len() < channels {
            for key in std::iter::once(&r.key).chain(&r.also) {
                sent.insert(key.clone(), now);
            }
        }
    }
    if preview && due.is_empty() {
        println!("Nothing to send right now.");
    }
    if !preview {
        sent.retain(|_, t| now - *t < KEEP_SENT_DAYS * 86400);
        db.set_meta("reminders_sent", &serde_json::to_string(&sent)?)?;
        db.set_meta("last_remind", &now.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 10, h, m, 0).unwrap()
    }

    fn task(id: i64, due: i64) -> Task {
        Task { id, title: format!("HW {id}"), due: Some(due), ..Default::default() }
    }

    fn event(id: i64, source: &str, start: i64) -> Event {
        Event { id, title: "Mathe".into(), start, end: start + 3000, location: String::new(), notes: String::new(), source: source.into(), ext_id: None, course: String::new() }
    }

    #[test]
    fn deadlines_send_only_the_closest_warning_once() {
        let cfg = Config::default();
        let now = at(12, 0);
        let ts = now.timestamp();
        let mut sent = HashMap::new();
        // Added late: 3 hours left crosses both 24 h and 6 h, so one message says 3 h.
        let tasks = vec![task(1, ts + 3 * 3600), task(2, ts + 30 * 3600), Task { done: true, ..task(3, ts + 3600) }];
        let got = collect(&cfg, now, &tasks, &[], 60, &sent);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].push.title, "Due in 3 h: HW 1");
        assert!(got[0].push.urgent);
        assert_eq!(got[0].push.buttons[0].1, "done 1");
        for k in std::iter::once(&got[0].key).chain(&got[0].also) {
            sent.insert(k.clone(), ts);
        }
        assert!(collect(&cfg, now, &tasks, &[], 60, &sent).is_empty(), "nothing twice");
        // Under 2 hours left, the last warning comes.
        let later = now + Duration::minutes(70);
        let got = collect(&cfg, later, &tasks, &[], 60, &sent);
        assert_eq!(got.len(), 1);
        assert!(got[0].push.title.starts_with("Due in 110 min"), "{}", got[0].push.title);
    }

    #[test]
    fn a_moved_deadline_warns_again() {
        let cfg = Config::default();
        let now = at(12, 0);
        let ts = now.timestamp();
        let first = collect(&cfg, now, &[task(1, ts + 3600)], &[], 60, &HashMap::new());
        let sent: HashMap<String, i64> = first.iter().map(|r| (r.key.clone(), ts)).collect();
        assert_eq!(collect(&cfg, now, &[task(1, ts + 7200)], &[], 60, &sent).len(), 1);
    }

    #[test]
    fn quiet_hours_wrap_past_midnight_and_hold_everything() {
        let cfg = Config::default();
        assert!(is_quiet(&cfg, at(23, 0)) && is_quiet(&cfg, at(6, 59)));
        assert!(!is_quiet(&cfg, at(7, 0)) && !is_quiet(&cfg, at(21, 59)));
        let now = at(23, 0);
        assert!(collect(&cfg, now, &[task(1, now.timestamp() + 600)], &[], 0, &HashMap::new()).is_empty());
        let off = Config { quiet_hours: String::new(), ..Config::default() };
        assert!(!is_quiet(&off, at(3, 0)));
    }

    #[test]
    fn study_sessions_get_a_heads_up_but_not_a_late_one() {
        let cfg = Config::default();
        let now = at(15, 0);
        let ts = now.timestamp();
        let events = vec![
            event(1, crate::planner::SOURCE_STUDY, ts + 8 * 60),
            event(2, crate::planner::SOURCE_STUDY, ts + 3600),
            event(3, crate::planner::SOURCE_STUDY, ts - 20 * 60),
            event(4, "", ts + 5 * 60),
        ];
        let got = collect(&cfg, now, &[], &events, 60, &HashMap::new());
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].push.title.starts_with("Study session at 15:08"));
    }

    #[test]
    fn the_nudge_comes_once_after_its_time_only_when_you_havent_studied() {
        let cfg = Config::default();
        let tasks = |now: DateTime<Local>| vec![task(1, now.timestamp() + 3 * 86400)];
        let before = at(16, 0);
        assert!(collect(&cfg, before, &tasks(before), &[], 0, &HashMap::new()).is_empty());
        let now = at(17, 30);
        let got = collect(&cfg, now, &tasks(now), &[], 0, &HashMap::new());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].push.title, "Time to study");
        assert!(got[0].push.body.contains("HW 1"));
        assert!(collect(&cfg, now, &tasks(now), &[], 45, &HashMap::new()).is_empty(), "studied already");
        let sent: HashMap<String, i64> = got.iter().map(|r| (r.key.clone(), 0)).collect();
        assert!(collect(&cfg, now, &tasks(now), &[], 0, &sent).is_empty());
        let late = at(21, 0);
        assert!(collect(&cfg, late, &tasks(late), &[], 0, &HashMap::new()).is_empty(), "too late for a nudge");
        let off = Config { remind_nudge_time: String::new(), ..Config::default() };
        assert!(collect(&off, now, &tasks(now), &[], 0, &HashMap::new()).is_empty());
    }

    #[test]
    fn tests_tomorrow_are_announced_in_the_evening() {
        let cfg = Config::default();
        let now = at(19, 0);
        let exam = Event { ext_id: Some("untis:9".into()), ..event(1, SOURCE_UNTIS, at(8, 0).timestamp() + 86400) };
        let study = event(2, crate::planner::SOURCE_STUDY, at(15, 0).timestamp());
        let got = collect(&cfg, now, &[], &[exam, study], 60, &HashMap::new());
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].push.title, "Test tomorrow: Mathe");
        assert!(got[0].push.body.contains("1 study session"));
    }

    #[test]
    fn time_left_reads_naturally() {
        assert_eq!(left(30), "1 min");
        assert_eq!(left(40 * 60), "40 min");
        assert_eq!(left(5 * 3600 + 100), "5 h");
        assert_eq!(left(3 * 86400), "3 days");
    }
}
