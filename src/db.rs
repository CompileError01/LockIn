use crate::timeutil::{date_of, now_ts, today};
use anyhow::Result;
use chrono::{Duration, NaiveDate};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS tasks(
  id INTEGER PRIMARY KEY, title TEXT NOT NULL, notes TEXT NOT NULL DEFAULT '', due INTEGER,
  priority INTEGER NOT NULL DEFAULT 0, priority_locked INTEGER NOT NULL DEFAULT 0,
  done INTEGER NOT NULL DEFAULT 0, done_by TEXT, done_at INTEGER,
  source TEXT NOT NULL DEFAULT 'local', ext_id TEXT UNIQUE, url TEXT NOT NULL DEFAULT '',
  course TEXT NOT NULL DEFAULT '', action TEXT NOT NULL DEFAULT '', ai_note TEXT NOT NULL DEFAULT '',
  effort_min INTEGER, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS events(
  id INTEGER PRIMARY KEY, title TEXT NOT NULL, start INTEGER NOT NULL, end INTEGER NOT NULL,
  location TEXT NOT NULL DEFAULT '', notes TEXT NOT NULL DEFAULT '', source TEXT NOT NULL DEFAULT 'local',
  ext_id TEXT UNIQUE, course TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS habits(id INTEGER PRIMARY KEY, name TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS habit_log(
  habit_id INTEGER NOT NULL REFERENCES habits(id) ON DELETE CASCADE, day TEXT NOT NULL, PRIMARY KEY(habit_id, day));
CREATE TABLE IF NOT EXISTS grades(
  course_id INTEGER PRIMARY KEY, course TEXT NOT NULL, grade TEXT NOT NULL, current INTEGER NOT NULL, items TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS focus(
  id INTEGER PRIMARY KEY, task_id INTEGER, title TEXT NOT NULL, finished_at INTEGER NOT NULL, minutes INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS briefings(id INTEGER PRIMARY KEY, created_at INTEGER NOT NULL, headline TEXT NOT NULL, plan TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS inbox(subject TEXT NOT NULL, message TEXT NOT NULL, time INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

#[derive(Clone, Debug, Default, Serialize)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub notes: String,
    pub due: Option<i64>,
    pub priority: i64,
    pub priority_locked: bool,
    pub done: bool,
    pub done_by: Option<String>,
    pub done_at: Option<i64>,
    pub source: String,
    pub ext_id: Option<String>,
    pub url: String,
    pub course: String,
    pub action: String,
    pub ai_note: String,
    pub effort_min: Option<i64>,
    pub created_at: i64,
}

impl Task {
    pub fn is_moodle(&self) -> bool {
        self.source == "moodle"
    }
}

const TASK_COLS: &str = "id,title,notes,due,priority,priority_locked,done,done_by,done_at,source,ext_id,url,course,action,ai_note,effort_min,created_at";

fn task_row(r: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get(0)?,
        title: r.get(1)?,
        notes: r.get(2)?,
        due: r.get(3)?,
        priority: r.get(4)?,
        priority_locked: r.get(5)?,
        done: r.get(6)?,
        done_by: r.get(7)?,
        done_at: r.get(8)?,
        source: r.get(9)?,
        ext_id: r.get(10)?,
        url: r.get(11)?,
        course: r.get(12)?,
        action: r.get(13)?,
        ai_note: r.get(14)?,
        effort_min: r.get(15)?,
        created_at: r.get(16)?,
    })
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Event {
    pub id: i64,
    pub title: String,
    pub start: i64,
    pub end: i64,
    pub location: String,
    pub notes: String,
    pub source: String,
    pub ext_id: Option<String>,
    pub course: String,
}

#[derive(Clone, Debug)]
pub struct Habit {
    pub id: i64,
    pub name: String,
    pub days: HashSet<NaiveDate>,
}

impl Habit {
    /// Days in a row, ending today, or yesterday while today is still open.
    pub fn streak(&self) -> i64 {
        let mut day = today();
        if !self.days.contains(&day) {
            day -= Duration::days(1);
        }
        let mut n = 0;
        while self.days.contains(&day) {
            n += 1;
            day -= Duration::days(1);
        }
        n
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GradeItem {
    pub name: String,
    pub grade: String,
    pub range: String,
    pub pct: String,
    pub feedback: String,
    pub total: bool,
}

#[derive(Clone, Debug)]
pub struct Grade {
    pub course: String,
    pub grade: String,
    pub current: bool,
    pub items: Vec<GradeItem>,
}

#[derive(Clone, Debug)]
pub struct Briefing {
    pub created_at: i64,
    pub headline: String,
    pub plan: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxItem {
    pub subject: String,
    pub message: String,
    pub time: i64,
}

#[derive(Debug, Deserialize)]
pub struct RemoteTask {
    pub ext_id: String,
    pub title: String,
    pub course: String,
    pub due: i64,
    pub url: String,
    pub action: String,
    pub notes: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoteEvent {
    pub ext_id: String,
    pub title: String,
    pub course: String,
    pub start: i64,
    pub end: i64,
    pub location: String,
    pub notes: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoteGrade {
    pub course_id: i64,
    pub course: String,
    pub grade: String,
    pub current: bool,
    pub items: Vec<GradeItem>,
}

#[derive(Debug, Deserialize)]
pub struct Snapshot {
    pub window_from: i64,
    pub tasks: Vec<RemoteTask>,
    pub events: Vec<RemoteEvent>,
    pub grades: Vec<RemoteGrade>,
    pub inbox: Vec<InboxItem>,
}

#[derive(Debug, Default, PartialEq)]
pub struct SyncSummary {
    pub added: usize,
    pub updated: usize,
    pub completed: usize,
}

pub struct Db {
    pub c: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        let c = Connection::open(path)?;
        c.busy_timeout(std::time::Duration::from_secs(10))?;
        c.pragma_update(None, "journal_mode", "WAL")?;
        c.pragma_update(None, "foreign_keys", "ON")?;
        c.execute_batch(SCHEMA)?;
        Ok(Db { c })
    }

    /// Changes whenever another connection commits, so the TUI can pick up background writes.
    pub fn data_version(&self) -> i64 {
        self.c.pragma_query_value(None, "data_version", |r| r.get(0)).unwrap_or(0)
    }

    // ---- tasks ------------------------------------------------------------

    pub fn tasks(&self) -> Result<Vec<Task>> {
        let sql = format!("SELECT {TASK_COLS} FROM tasks ORDER BY done, due IS NULL, due, priority DESC, id");
        let mut st = self.c.prepare(&sql)?;
        let rows = st.query_map([], task_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn task(&self, id: i64) -> Result<Option<Task>> {
        let sql = format!("SELECT {TASK_COLS} FROM tasks WHERE id=?1");
        Ok(self.c.query_row(&sql, [id], task_row).optional()?)
    }

    pub fn add_task(&self, title: &str, due: Option<i64>, priority: i64, course: &str, effort: Option<i64>) -> Result<i64> {
        self.c.execute(
            "INSERT INTO tasks(title,due,priority,priority_locked,course,effort_min,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![title, due, priority, priority > 0, course, effort, now_ts()],
        )?;
        Ok(self.c.last_insert_rowid())
    }

    /// Writes a full snapshot back, which is what undo uses.
    pub fn put_task(&self, t: &Task) -> Result<()> {
        self.c.execute(
            "INSERT OR REPLACE INTO tasks(id,title,notes,due,priority,priority_locked,done,done_by,done_at,source,ext_id,url,course,action,ai_note,effort_min,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            params![t.id, t.title, t.notes, t.due, t.priority, t.priority_locked, t.done, t.done_by, t.done_at, t.source, t.ext_id, t.url, t.course, t.action, t.ai_note, t.effort_min, t.created_at],
        )?;
        Ok(())
    }

    pub fn delete_task(&self, id: i64) -> Result<()> {
        self.c.execute("DELETE FROM tasks WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn set_done(&self, id: i64, done: bool) -> Result<()> {
        let (by, at) = if done { (Some("user"), Some(now_ts())) } else { (None, None) };
        self.c.execute("UPDATE tasks SET done=?2, done_by=?3, done_at=?4 WHERE id=?1", params![id, done, by, at])?;
        Ok(())
    }

    pub fn set_title(&self, id: i64, title: &str) -> Result<()> {
        self.c.execute("UPDATE tasks SET title=?2 WHERE id=?1", params![id, title])?;
        Ok(())
    }

    pub fn set_due(&self, id: i64, due: Option<i64>) -> Result<()> {
        self.c.execute("UPDATE tasks SET due=?2 WHERE id=?1", params![id, due])?;
        Ok(())
    }

    pub fn set_notes(&self, id: i64, notes: &str) -> Result<()> {
        self.c.execute("UPDATE tasks SET notes=?2 WHERE id=?1", params![id, notes])?;
        Ok(())
    }

    /// A priority you set yourself is locked, so the daily briefing won't change it.
    pub fn set_priority(&self, id: i64, priority: i64) -> Result<()> {
        self.c.execute("UPDATE tasks SET priority=?2, priority_locked=1 WHERE id=?1", params![id, priority.clamp(0, 3)])?;
        Ok(())
    }

    pub fn apply_ai(&self, id: i64, priority: i64, effort: i64, note: &str) -> Result<()> {
        self.c.execute(
            "UPDATE tasks SET ai_note=?2, effort_min=?3, priority=CASE WHEN priority_locked=1 THEN priority ELSE ?4 END WHERE id=?1 AND done=0",
            params![id, note, effort.max(0), priority.clamp(0, 3)],
        )?;
        Ok(())
    }

    // ---- events -----------------------------------------------------------

    pub fn events(&self) -> Result<Vec<Event>> {
        let mut st = self.c.prepare("SELECT id,title,start,end,location,notes,source,ext_id,course FROM events ORDER BY start")?;
        let rows = st
            .query_map([], |r| {
                Ok(Event {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    start: r.get(2)?,
                    end: r.get(3)?,
                    location: r.get(4)?,
                    notes: r.get(5)?,
                    source: r.get(6)?,
                    ext_id: r.get(7)?,
                    course: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn add_event(&self, title: &str, start: i64, end: i64) -> Result<i64> {
        self.c.execute("INSERT INTO events(title,start,end) VALUES(?1,?2,?3)", params![title, start, end])?;
        Ok(self.c.last_insert_rowid())
    }

    pub fn put_event(&self, e: &Event) -> Result<()> {
        self.c.execute(
            "INSERT OR REPLACE INTO events(id,title,start,end,location,notes,source,ext_id,course) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![e.id, e.title, e.start, e.end, e.location, e.notes, e.source, e.ext_id, e.course],
        )?;
        Ok(())
    }

    pub fn delete_event(&self, id: i64) -> Result<()> {
        self.c.execute("DELETE FROM events WHERE id=?1", [id])?;
        Ok(())
    }

    // ---- habits -----------------------------------------------------------

    pub fn habits(&self) -> Result<Vec<Habit>> {
        let mut st = self.c.prepare("SELECT id,name FROM habits ORDER BY id")?;
        let base: Vec<(i64, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut log = self.c.prepare("SELECT day FROM habit_log WHERE habit_id=?1")?;
        let mut out = Vec::new();
        for (id, name) in base {
            let days: Vec<String> = log.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            let days = days.iter().filter_map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).collect();
            out.push(Habit { id, name, days });
        }
        Ok(out)
    }

    pub fn add_habit(&self, name: &str) -> Result<()> {
        self.c.execute("INSERT INTO habits(name,created_at) VALUES(?1,?2)", params![name, now_ts()])?;
        Ok(())
    }

    pub fn delete_habit(&self, id: i64) -> Result<()> {
        self.c.execute("DELETE FROM habits WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn toggle_habit(&self, id: i64, day: NaiveDate) -> Result<()> {
        let d = day.format("%Y-%m-%d").to_string();
        let removed = self.c.execute("DELETE FROM habit_log WHERE habit_id=?1 AND day=?2", params![id, d])?;
        if removed == 0 {
            self.c.execute("INSERT INTO habit_log(habit_id,day) VALUES(?1,?2)", params![id, d])?;
        }
        Ok(())
    }

    // ---- misc -------------------------------------------------------------

    pub fn grades(&self) -> Result<Vec<Grade>> {
        let mut st = self.c.prepare("SELECT course,grade,current,items FROM grades ORDER BY current DESC, course")?;
        let rows = st
            .query_map([], |r| {
                let items: String = r.get(3)?;
                Ok(Grade {
                    course: r.get(0)?,
                    grade: r.get(1)?,
                    current: r.get(2)?,
                    items: serde_json::from_str(&items).unwrap_or_default(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn inbox(&self) -> Result<Vec<InboxItem>> {
        let mut st = self.c.prepare("SELECT subject,message,time FROM inbox ORDER BY time DESC")?;
        let rows = st
            .query_map([], |r| Ok(InboxItem { subject: r.get(0)?, message: r.get(1)?, time: r.get(2)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn log_focus(&self, task_id: Option<i64>, title: &str, minutes: i64) -> Result<()> {
        self.c.execute(
            "INSERT INTO focus(task_id,title,finished_at,minutes) VALUES(?1,?2,?3,?4)",
            params![task_id, title, now_ts(), minutes],
        )?;
        Ok(())
    }

    pub fn focus_minutes_today(&self) -> Result<i64> {
        let mut st = self.c.prepare("SELECT finished_at,minutes FROM focus ORDER BY id DESC LIMIT 200")?;
        let rows: Vec<(i64, i64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows.iter().filter(|(ts, _)| date_of(*ts) == today()).map(|(_, m)| m).sum())
    }

    pub fn latest_briefing(&self) -> Result<Option<Briefing>> {
        Ok(self
            .c
            .query_row("SELECT created_at,headline,plan FROM briefings ORDER BY id DESC LIMIT 1", [], |r| {
                Ok(Briefing { created_at: r.get(0)?, headline: r.get(1)?, plan: r.get(2)? })
            })
            .optional()?)
    }

    pub fn add_briefing(&self, headline: &str, plan: &str) -> Result<()> {
        self.c.execute("INSERT INTO briefings(created_at,headline,plan) VALUES(?1,?2,?3)", params![now_ts(), headline, plan])?;
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        self.c.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)).optional().ok().flatten()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.c.execute("INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)", params![key, value])?;
        Ok(())
    }

    // ---- Moodle merge -----------------------------------------------------

    /// Moodle's timeline is the source of truth for open work. A task that disappears from it
    /// was submitted, so it closes itself. One you ticked off yourself stays ticked off.
    pub fn apply_snapshot(&mut self, s: &Snapshot) -> Result<SyncSummary> {
        let mut sum = SyncSummary::default();
        let now = now_ts();
        let tx = self.c.transaction()?;
        for r in &s.tasks {
            let existing: Option<(bool, Option<String>)> = tx
                .query_row("SELECT done,done_by FROM tasks WHERE ext_id=?1", [&r.ext_id], |x| Ok((x.get(0)?, x.get(1)?)))
                .optional()?;
            match existing {
                None => {
                    tx.execute(
                        "INSERT INTO tasks(title,notes,due,source,ext_id,url,course,action,created_at) VALUES(?1,?2,?3,'moodle',?4,?5,?6,?7,?8)",
                        params![r.title, r.notes, r.due, r.ext_id, r.url, r.course, r.action, now],
                    )?;
                    sum.added += 1;
                }
                Some((done, by)) => {
                    let reopen = done && by.as_deref() == Some("moodle");
                    tx.execute(
                        "UPDATE tasks SET title=?2,notes=?3,due=?4,url=?5,course=?6,action=?7,
                         done=CASE WHEN ?8 THEN 0 ELSE done END,
                         done_by=CASE WHEN ?8 THEN NULL ELSE done_by END,
                         done_at=CASE WHEN ?8 THEN NULL ELSE done_at END WHERE ext_id=?1",
                        params![r.ext_id, r.title, r.notes, r.due, r.url, r.course, r.action, reopen],
                    )?;
                    sum.updated += 1;
                }
            }
        }
        let seen: HashSet<&str> = s.tasks.iter().map(|t| t.ext_id.as_str()).collect();
        let open: Vec<(i64, String)> = {
            // Only tasks inside the lookback window count: older ones can't be seen either way.
            let mut st = tx.prepare("SELECT id,ext_id FROM tasks WHERE source='moodle' AND done=0 AND due>=?1")?;
            let rows = st.query_map([s.window_from], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            rows
        };
        for (id, ext) in open {
            if !seen.contains(ext.as_str()) {
                tx.execute("UPDATE tasks SET done=1,done_by='moodle',done_at=?2 WHERE id=?1", params![id, now])?;
                sum.completed += 1;
            }
        }

        let seen_ev: HashSet<&str> = s.events.iter().map(|e| e.ext_id.as_str()).collect();
        let stale: Vec<(i64, String)> = {
            let mut st = tx.prepare("SELECT id,ext_id FROM events WHERE source='moodle' AND start>=?1")?;
            let rows = st.query_map([now], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            rows
        };
        for (id, ext) in stale {
            if !seen_ev.contains(ext.as_str()) {
                tx.execute("DELETE FROM events WHERE id=?1", [id])?;
            }
        }
        for e in &s.events {
            tx.execute(
                "INSERT INTO events(title,start,end,location,notes,source,ext_id,course) VALUES(?1,?2,?3,?4,?5,'moodle',?6,?7)
                 ON CONFLICT(ext_id) DO UPDATE SET title=excluded.title,start=excluded.start,end=excluded.end,
                 location=excluded.location,notes=excluded.notes,course=excluded.course",
                params![e.title, e.start, e.end, e.location, e.notes, e.ext_id, e.course],
            )?;
        }

        tx.execute("DELETE FROM grades", [])?;
        for g in &s.grades {
            tx.execute(
                "INSERT INTO grades(course_id,course,grade,current,items) VALUES(?1,?2,?3,?4,?5)",
                params![g.course_id, g.course, g.grade, g.current, serde_json::to_string(&g.items)?],
            )?;
        }
        tx.execute("DELETE FROM inbox", [])?;
        for n in &s.inbox {
            tx.execute("INSERT INTO inbox(subject,message,time) VALUES(?1,?2,?3)", params![n.subject, n.message, n.time])?;
        }
        tx.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('last_sync',?1)", [now.to_string()])?;
        tx.commit()?;
        Ok(sum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        let c = Connection::open_in_memory().unwrap();
        c.pragma_update(None, "foreign_keys", "ON").unwrap();
        c.execute_batch(SCHEMA).unwrap();
        Db { c }
    }

    fn remote(id: &str, due: i64) -> RemoteTask {
        RemoteTask {
            ext_id: id.into(),
            title: format!("task {id}"),
            course: "AM".into(),
            due,
            url: "u".into(),
            action: "Add submission".into(),
            notes: String::new(),
        }
    }

    fn snap(tasks: Vec<RemoteTask>, window_from: i64) -> Snapshot {
        Snapshot { window_from, tasks, events: vec![], grades: vec![], inbox: vec![] }
    }

    #[test]
    fn sync_adds_updates_and_closes_submitted_work() {
        let mut d = db();
        let due = now_ts() + 3600;
        let s = d.apply_snapshot(&snap(vec![remote("a", due), remote("b", due)], 0)).unwrap();
        assert_eq!(s, SyncSummary { added: 2, updated: 0, completed: 0 });

        let s = d.apply_snapshot(&snap(vec![remote("a", due + 60)], 0)).unwrap();
        assert_eq!(s, SyncSummary { added: 0, updated: 1, completed: 1 });
        let tasks = d.tasks().unwrap();
        let a = tasks.iter().find(|t| t.ext_id.as_deref() == Some("a")).unwrap();
        let b = tasks.iter().find(|t| t.ext_id.as_deref() == Some("b")).unwrap();
        assert_eq!(a.due, Some(due + 60));
        assert!(b.done && b.done_by.as_deref() == Some("moodle"));

        // Reappearing on Moodle (say, a reverted submission) reopens it.
        d.apply_snapshot(&snap(vec![remote("a", due), remote("b", due)], 0)).unwrap();
        assert!(!d.tasks().unwrap().iter().any(|t| t.done));
    }

    #[test]
    fn sync_keeps_your_own_checkmark_and_ignores_tasks_outside_window() {
        let mut d = db();
        let due = now_ts() + 3600;
        d.apply_snapshot(&snap(vec![remote("a", due), remote("old", 500)], 0)).unwrap();
        let id = d.tasks().unwrap().iter().find(|t| t.ext_id.as_deref() == Some("a")).unwrap().id;
        d.set_done(id, true).unwrap();
        d.apply_snapshot(&snap(vec![remote("a", due)], 1000)).unwrap();
        let tasks = d.tasks().unwrap();
        assert!(tasks.iter().find(|t| t.id == id).unwrap().done, "user checkmark survives sync");
        assert!(!tasks.iter().find(|t| t.ext_id.as_deref() == Some("old")).unwrap().done, "older than the window stays open");
    }

    #[test]
    fn ai_respects_locked_priority_and_done_tasks() {
        let d = db();
        let a = d.add_task("mine", None, 0, "", None).unwrap();
        let b = d.add_task("free", None, 0, "", None).unwrap();
        d.set_priority(a, 1).unwrap();
        d.apply_ai(a, 3, 30, "n").unwrap();
        d.apply_ai(b, 3, 30, "n").unwrap();
        d.apply_ai(9999, 3, 30, "ignored id").unwrap();
        assert_eq!(d.task(a).unwrap().unwrap().priority, 1);
        let free = d.task(b).unwrap().unwrap();
        assert_eq!((free.priority, free.effort_min, free.ai_note.as_str()), (3, Some(30), "n"));
    }

    #[test]
    fn habit_streak_counts_back_from_today_or_yesterday() {
        let d = db();
        d.add_habit("read").unwrap();
        let h = d.habits().unwrap()[0].id;
        d.toggle_habit(h, today() - Duration::days(1)).unwrap();
        d.toggle_habit(h, today() - Duration::days(2)).unwrap();
        d.toggle_habit(h, today() - Duration::days(4)).unwrap();
        assert_eq!(d.habits().unwrap()[0].streak(), 2);
        d.toggle_habit(h, today()).unwrap();
        assert_eq!(d.habits().unwrap()[0].streak(), 3);
        d.toggle_habit(h, today()).unwrap();
        d.toggle_habit(h, today() - Duration::days(1)).unwrap();
        assert_eq!(d.habits().unwrap()[0].streak(), 0);
    }
}
