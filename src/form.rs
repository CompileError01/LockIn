//! The pop-up form for adding and editing calendar items.

use crate::db::{Event, Task};
use crate::timeutil::{date_of, local, parse_minutes, parse_when, ts_of};
use anyhow::Result;
use chrono::{Duration, Local, NaiveDate, NaiveTime};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Target {
    Event(Option<i64>),
    Task(Option<i64>),
}

impl Target {
    pub fn is_new(&self) -> bool {
        matches!(self, Target::Event(None) | Target::Task(None))
    }
}

#[derive(Clone, Debug)]
pub struct Field {
    pub label: &'static str,
    pub value: Vec<char>,
}

#[derive(Clone, Debug)]
pub struct Form {
    pub target: Target,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub cursor: usize,
    /// Like a text box in a browser: the first key you type replaces what's there.
    pub fresh: bool,
    pub error: Option<String>,
}

pub const TITLE: usize = 0;
pub const DATE: usize = 1;
pub const TIME: usize = 2;
pub const LENGTH: usize = 3;
pub const LOCATION: usize = 4;
pub const NOTES: usize = 5;

fn field(label: &'static str, value: &str) -> Field {
    Field { label, value: value.chars().collect() }
}

fn date_text(d: NaiveDate) -> String {
    d.format("%d.%m.%Y").to_string()
}

fn length_text(minutes: i64) -> String {
    if minutes % 60 == 0 { format!("{}h", minutes / 60) } else { format!("{minutes}m") }
}

fn event_fields(title: &str, date: &str, time: &str, length: &str, location: &str, notes: &str) -> Vec<Field> {
    vec![field("Title", title), field("Date", date), field("Time", time), field("Length", length), field("Location", location), field("Notes", notes)]
}

fn task_fields(title: &str, date: &str, time: &str) -> Vec<Field> {
    vec![field("Title", title), field("Date", date), field("Due at", time)]
}

impl Form {
    fn build(target: Target, fields: Vec<Field>) -> Form {
        let cursor = fields[TITLE].value.len();
        let fresh = !target.is_new();
        Form { target, fields, focus: TITLE, cursor, fresh, error: None }
    }

    pub fn new_event(day: NaiveDate) -> Form {
        Form::build(Target::Event(None), event_fields("", &date_text(day), "09:00", "1h", "", ""))
    }

    pub fn new_task(day: NaiveDate) -> Form {
        Form::build(Target::Task(None), task_fields("", &date_text(day), "23:59"))
    }

    pub fn edit_event(e: &Event) -> Form {
        let start = local(e.start);
        let minutes = ((e.end - e.start) / 60).max(1);
        let fields = event_fields(&e.title, &date_text(start.date_naive()), &start.format("%H:%M").to_string(), &length_text(minutes), &e.location, &e.notes);
        Form::build(Target::Event(Some(e.id)), fields)
    }

    pub fn edit_task(t: &Task, day: NaiveDate) -> Form {
        let due = t.due.map(local);
        let date = due.map_or(day, |d| d.date_naive());
        let time = due.map_or("23:59".to_string(), |d| d.format("%H:%M").to_string());
        Form::build(Target::Task(Some(t.id)), task_fields(&t.title, &date_text(date), &time))
    }

    /// Switches a new item between event and task, keeping what you've typed.
    pub fn toggle_type(&mut self) {
        let (title, date, time) = (self.text(TITLE), self.text(DATE), self.text(TIME));
        match self.target {
            Target::Event(None) => {
                self.target = Target::Task(None);
                self.fields = task_fields(&title, &date, if time == "09:00" { "23:59" } else { &time });
            }
            Target::Task(None) => {
                self.target = Target::Event(None);
                self.fields = event_fields(&title, &date, if time == "23:59" { "09:00" } else { &time }, "1h", "", "");
            }
            _ => return,
        }
        self.focus = self.focus.min(self.fields.len() - 1);
        self.place_cursor();
    }

    pub fn text(&self, i: usize) -> String {
        self.fields.get(i).map_or(String::new(), |f| f.value.iter().collect())
    }

    fn place_cursor(&mut self) {
        self.cursor = self.fields[self.focus].value.len();
        self.fresh = !self.fields[self.focus].value.is_empty();
    }

    pub fn step(&mut self, dir: isize) {
        self.focus = (self.focus as isize + dir).rem_euclid(self.fields.len() as isize) as usize;
        self.place_cursor();
    }

    fn take_fresh(&mut self) {
        if self.fresh {
            self.fields[self.focus].value.clear();
            self.cursor = 0;
            self.fresh = false;
        }
    }

    pub fn insert(&mut self, c: char) {
        self.take_fresh();
        self.fields[self.focus].value.insert(self.cursor, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.fresh {
            return self.take_fresh();
        }
        if self.cursor > 0 {
            self.cursor -= 1;
            self.fields[self.focus].value.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        self.fresh = false;
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.fresh = false;
        self.cursor = (self.cursor + 1).min(self.fields[self.focus].value.len());
    }

    pub fn clear_line(&mut self) {
        self.fields[self.focus].value.clear();
        self.cursor = 0;
        self.fresh = false;
    }

    fn fail(&mut self, field: usize, message: String) -> anyhow::Error {
        self.focus = field;
        self.place_cursor();
        anyhow::anyhow!(message)
    }

    /// The date as typed, in words, so you can see what `fri` means before saving.
    pub fn date_preview(&self) -> Option<String> {
        parse_day(&self.text(DATE)).map(|d| d.format("%A %d %B %Y").to_string())
    }

    pub fn parse(&mut self) -> Result<Parsed> {
        let title = self.text(TITLE).trim().to_string();
        if title.is_empty() {
            return Err(self.fail(TITLE, "Give it a title".into()));
        }
        let date_input = self.text(DATE);
        let Some(day) = parse_day(&date_input) else {
            return Err(self.fail(DATE, format!("Couldn't read the date '{date_input}'. Try fri, tom, +3d or 14.11.")));
        };
        let time_input = self.text(TIME);
        let Some(time) = parse_clock(&time_input) else {
            return Err(self.fail(TIME, format!("Couldn't read the time '{time_input}'. Try 14:00 or 9")));
        };
        let start = ts_of(day, time);
        match self.target {
            Target::Task(_) => Ok(Parsed { title, start, end: start, location: String::new(), notes: String::new() }),
            Target::Event(_) => {
                let length_input = self.text(LENGTH);
                let minutes = parse_minutes(length_input.trim()).filter(|m| *m > 0);
                let Some(minutes) = minutes else {
                    return Err(self.fail(LENGTH, format!("Couldn't read the length '{length_input}'. Try 45m, 1h or 1.5h")));
                };
                Ok(Parsed {
                    title,
                    start,
                    end: start + minutes * 60,
                    location: self.text(LOCATION).trim().to_string(),
                    notes: self.text(NOTES).trim().to_string(),
                })
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Parsed {
    pub title: String,
    pub start: i64,
    pub end: i64,
    pub location: String,
    pub notes: String,
}

pub fn parse_day(text: &str) -> Option<NaiveDate> {
    parse_when(text, NaiveTime::MIN, Local::now()).map(date_of)
}

/// Accepts `14:00`, `14.30`, `9` and `0930`.
pub fn parse_clock(text: &str) -> Option<NaiveTime> {
    let t = text.trim().replace('.', ":");
    if let Ok(time) = NaiveTime::parse_from_str(&t, "%H:%M") {
        return Some(time);
    }
    if !t.is_empty() && t.len() <= 4 && t.chars().all(|c| c.is_ascii_digit()) {
        let n: u32 = t.parse().ok()?;
        return if t.len() <= 2 { NaiveTime::from_hms_opt(n, 0, 0) } else { NaiveTime::from_hms_opt(n / 100, n % 100, 0) };
    }
    None
}

/// Moves a timestamp by whole days, keeping the same clock time across daylight-saving changes.
pub fn shift_days(ts: i64, days: i64) -> i64 {
    let at = local(ts);
    ts_of(at.date_naive() + Duration::days(days), at.time())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()
    }

    fn type_in(form: &mut Form, text: &str) {
        text.chars().for_each(|c| form.insert(c));
    }

    #[test]
    fn a_new_event_needs_only_a_title() {
        let mut f = Form::new_event(day());
        assert!(f.parse().is_err(), "the title is required");
        assert_eq!(f.focus, TITLE);
        type_in(&mut f, "Exam");
        let p = f.parse().unwrap();
        assert_eq!(p.title, "Exam");
        assert_eq!(p.start, ts_of(day(), NaiveTime::from_hms_opt(9, 0, 0).unwrap()));
        assert_eq!(p.end - p.start, 3600);
    }

    #[test]
    fn typing_replaces_a_prefilled_field_but_arrows_keep_it() {
        let mut f = Form::new_event(day());
        f.step(1);
        f.step(1);
        assert_eq!(f.focus, TIME);
        type_in(&mut f, "14:30");
        assert_eq!(f.text(TIME), "14:30");
        f.step(1);
        f.left();
        f.insert('5');
        assert_eq!(f.text(LENGTH), "15h", "after an arrow key, typing inserts");
        f.step(1);
        f.step(-1);
        f.backspace();
        assert_eq!(f.text(LENGTH), "", "backspace on a selected field clears it");
    }

    #[test]
    fn bad_input_names_the_problem_and_moves_to_that_field() {
        let mut f = Form::new_event(day());
        type_in(&mut f, "Exam");
        f.fields[DATE].value = "someday".chars().collect();
        assert!(f.parse().unwrap_err().to_string().contains("someday"));
        assert_eq!(f.focus, DATE);
        f.fields[DATE].value = "20.11.2026".chars().collect();
        f.fields[TIME].value = "25:00".chars().collect();
        assert!(f.parse().is_err());
        assert_eq!(f.focus, TIME);
        f.fields[TIME].value = "9".chars().collect();
        f.fields[LENGTH].value = "0".chars().collect();
        assert!(f.parse().is_err());
        assert_eq!(f.focus, LENGTH);
    }

    #[test]
    fn editing_starts_from_the_current_values() {
        let start = ts_of(day(), NaiveTime::from_hms_opt(14, 0, 0).unwrap());
        let e = Event { id: 7, title: "Exam".into(), start, end: start + 5400, location: "R12".into(), notes: "bring ID".into(), ..Default::default() };
        let mut f = Form::edit_event(&e);
        assert_eq!(f.target, Target::Event(Some(7)));
        assert_eq!((f.text(DATE).as_str(), f.text(TIME).as_str(), f.text(LENGTH).as_str()), ("20.11.2026", "14:00", "90m"));
        let p = f.parse().unwrap();
        assert_eq!((p.start, p.end, p.location.as_str(), p.notes.as_str()), (start, start + 5400, "R12", "bring ID"));
    }

    #[test]
    fn new_items_can_switch_type_and_keep_the_title() {
        let mut f = Form::new_event(day());
        type_in(&mut f, "Essay");
        f.toggle_type();
        assert_eq!(f.target, Target::Task(None));
        assert_eq!((f.text(TITLE).as_str(), f.text(TIME).as_str()), ("Essay", "23:59"));
        f.toggle_type();
        assert_eq!(f.target, Target::Event(None));
        assert_eq!(f.text(TIME), "09:00");
        let mut existing = Form::edit_event(&Event { id: 1, title: "x".into(), ..Default::default() });
        existing.toggle_type();
        assert_eq!(existing.target, Target::Event(Some(1)), "saved items keep their type");
    }

    #[test]
    fn clock_formats_and_day_shifts() {
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0);
        assert_eq!(parse_clock("14:00"), t(14, 0));
        assert_eq!(parse_clock("14.30"), t(14, 30));
        assert_eq!(parse_clock("9"), t(9, 0));
        assert_eq!(parse_clock("0930"), t(9, 30));
        assert_eq!(parse_clock("25"), None);
        assert_eq!(parse_clock("late"), None);
        assert_eq!(parse_clock(""), None);

        // Across the end of daylight saving in Austria, the clock time stays the same.
        let before = ts_of(NaiveDate::from_ymd_opt(2026, 10, 24).unwrap(), t(9, 0).unwrap());
        let after = local(shift_days(before, 2));
        assert_eq!((after.day(), after.hour()), (26, 9));
    }
}
