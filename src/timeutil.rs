use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone, Weekday};

pub fn now_ts() -> i64 {
    Local::now().timestamp()
}

pub fn local(ts: i64) -> DateTime<Local> {
    Local.timestamp_opt(ts, 0).single().unwrap_or_else(Local::now)
}

pub fn date_of(ts: i64) -> NaiveDate {
    local(ts).date_naive()
}

pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

pub fn ts_of(date: NaiveDate, time: NaiveTime) -> i64 {
    let naive = date.and_time(time);
    Local.from_local_datetime(&naive).earliest().map(|d| d.timestamp()).unwrap_or(0)
}

pub fn fmt_abs(ts: i64) -> String {
    local(ts).format("%a %d %b %H:%M").to_string()
}

pub fn fmt_rel(ts: i64) -> String {
    fmt_rel_from(ts, now_ts())
}

pub fn fmt_rel_from(ts: i64, now: i64) -> String {
    let diff = ts - now;
    let abs = diff.abs();
    let text = if abs < 60 {
        return "now".into();
    } else if abs < 3600 {
        format!("{}m", abs / 60)
    } else if abs < 86400 {
        format!("{}h", abs / 3600)
    } else {
        format!("{}d", abs / 86400)
    };
    if diff > 0 { format!("in {text}") } else { format!("{text} ago") }
}

fn parse_time(tok: &str) -> Option<NaiveTime> {
    let (h, m) = tok.split_once(':')?;
    NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, 0)
}

fn weekday(tok: &str) -> Option<Weekday> {
    Some(match tok {
        "mon" | "monday" => Weekday::Mon,
        "tue" | "tues" | "tuesday" => Weekday::Tue,
        "wed" | "wednesday" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" => Weekday::Thu,
        "fri" | "friday" => Weekday::Fri,
        "sat" | "saturday" => Weekday::Sat,
        "sun" | "sunday" => Weekday::Sun,
        _ => return None,
    })
}

/// Accepts dotted dates as `14.11.`, `14.11` or `14.11.2026`, and ISO `2026-11-14`.
fn parse_date(tok: &str, now: NaiveDate) -> Option<NaiveDate> {
    if let Ok(d) = NaiveDate::parse_from_str(tok, "%Y-%m-%d") {
        return Some(d);
    }
    let parts: Vec<&str> = tok.trim_end_matches('.').split('.').collect();
    match parts[..] {
        [d, m] => {
            let (d, m) = (d.parse().ok()?, m.parse().ok()?);
            let this_year = NaiveDate::from_ymd_opt(now.year(), m, d)?;
            if this_year < now { NaiveDate::from_ymd_opt(now.year() + 1, m, d) } else { Some(this_year) }
        }
        [d, m, y] => NaiveDate::from_ymd_opt(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?),
        _ => None,
    }
}

/// Parses `fri`, `tom 18:00`, `+3d`, `+4h`, `14.11.`, `2026-11-14 08:00`, `21:00`.
/// A date with no time uses `default_time`; a time that already passed today means tomorrow.
pub fn parse_when(input: &str, default_time: NaiveTime, now: DateTime<Local>) -> Option<i64> {
    let input = input.trim().to_lowercase();
    if input.is_empty() {
        return None;
    }
    let today = now.date_naive();
    let (mut date, mut time) = (None, None);
    for tok in input.split_whitespace() {
        if let Some(rel) = tok.strip_prefix('+') {
            let unit = rel.chars().last()?;
            let n: i64 = rel[..rel.len() - unit.len_utf8()].parse().ok()?;
            match unit {
                'h' => return Some(now.timestamp() + n * 3600),
                'm' => return Some(now.timestamp() + n * 60),
                'd' => date = Some(today + Duration::days(n)),
                'w' => date = Some(today + Duration::weeks(n)),
                _ => return None,
            }
        } else if tok == "today" || tok == "tod" {
            date = Some(today);
        } else if tok == "tomorrow" || tok == "tom" {
            date = Some(today + Duration::days(1));
        } else if let Some(day) = weekday(tok) {
            let ahead = (day.num_days_from_monday() + 7 - today.weekday().num_days_from_monday()) % 7;
            date = Some(today + Duration::days(if ahead == 0 { 7 } else { ahead as i64 }));
        } else if let Some(t) = parse_time(tok) {
            time = Some(t);
        } else {
            date = Some(parse_date(tok, today)?);
        }
    }
    match (date, time) {
        (Some(d), t) => Some(ts_of(d, t.unwrap_or(default_time))),
        (None, Some(t)) => {
            let d = if t > now.time() { today } else { today + Duration::days(1) };
            Some(ts_of(d, t))
        }
        (None, None) => None,
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct QuickAdd {
    pub title: String,
    pub when: Option<String>,
    pub priority: i64,
    pub course: Option<String>,
    pub minutes: Option<i64>,
}

/// Parses `Read chapter 4 @fri 18:00 !! #AM ~45m`.
pub fn parse_quick(input: &str) -> QuickAdd {
    let mut q = QuickAdd::default();
    let mut title = Vec::new();
    let mut when: Vec<&str> = Vec::new();
    let mut in_when = false;
    for tok in input.split_whitespace() {
        if !tok.is_empty() && tok.chars().all(|c| c == '!') {
            q.priority = tok.len().min(3) as i64;
            in_when = false;
        } else if let Some(tag) = tok.strip_prefix('#').filter(|t| !t.is_empty()) {
            q.course = Some(tag.to_string());
            in_when = false;
        } else if let Some(mins) = tok.strip_prefix('~').and_then(parse_minutes) {
            q.minutes = Some(mins);
            in_when = false;
        } else if let Some(rest) = tok.strip_prefix('@') {
            in_when = true;
            if !rest.is_empty() {
                when.push(rest);
            }
        } else if in_when {
            when.push(tok);
        } else {
            title.push(tok);
        }
    }
    q.title = title.join(" ");
    if !when.is_empty() {
        q.when = Some(when.join(" "));
    }
    q
}

pub fn parse_minutes(s: &str) -> Option<i64> {
    if let Some(h) = s.strip_suffix('h') {
        return h.parse::<f64>().ok().map(|h| (h * 60.0) as i64);
    }
    s.strip_suffix('m').unwrap_or(s).parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-11-09 is a Monday.
    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 9, 12, 0, 0).unwrap()
    }
    fn eod() -> NaiveTime {
        NaiveTime::from_hms_opt(23, 59, 0).unwrap()
    }
    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> i64 {
        Local.with_ymd_and_hms(y, m, d, h, min, 0).unwrap().timestamp()
    }

    #[test]
    fn weekdays_and_keywords() {
        assert_eq!(parse_when("fri", eod(), now()), Some(at(2026, 11, 13, 23, 59)));
        assert_eq!(parse_when("mon", eod(), now()), Some(at(2026, 11, 16, 23, 59)));
        assert_eq!(parse_when("tom 18:00", eod(), now()), Some(at(2026, 11, 10, 18, 0)));
        assert_eq!(parse_when("today", eod(), now()), Some(at(2026, 11, 9, 23, 59)));
    }

    #[test]
    fn dates_and_relative() {
        assert_eq!(parse_when("14.11.", eod(), now()), Some(at(2026, 11, 14, 23, 59)));
        assert_eq!(parse_when("1.2", eod(), now()), Some(at(2027, 2, 1, 23, 59)));
        assert_eq!(parse_when("2026-12-01 08:15", eod(), now()), Some(at(2026, 12, 1, 8, 15)));
        assert_eq!(parse_when("+3d", eod(), now()), Some(at(2026, 11, 12, 23, 59)));
        assert_eq!(parse_when("+4h", eod(), now()), Some(at(2026, 11, 9, 16, 0)));
    }

    #[test]
    fn bare_time_rolls_to_tomorrow_once_passed() {
        assert_eq!(parse_when("21:00", eod(), now()), Some(at(2026, 11, 9, 21, 0)));
        assert_eq!(parse_when("08:00", eod(), now()), Some(at(2026, 11, 10, 8, 0)));
    }

    #[test]
    fn garbage_is_rejected() {
        assert_eq!(parse_when("whenever", eod(), now()), None);
        assert_eq!(parse_when("31.02.", eod(), now()), None);
        assert_eq!(parse_when("+xd", eod(), now()), None);
        assert_eq!(parse_when("", eod(), now()), None);
    }

    #[test]
    fn quick_add_extracts_all_fields() {
        let q = parse_quick("Read chapter 4 @fri 18:00 !! #AM ~45m");
        assert_eq!(q.title, "Read chapter 4");
        assert_eq!(q.when.as_deref(), Some("fri 18:00"));
        assert_eq!(q.priority, 2);
        assert_eq!(q.course.as_deref(), Some("AM"));
        assert_eq!(q.minutes, Some(45));
        assert_eq!(parse_quick("Call mum @ tom").when.as_deref(), Some("tom"));
        assert_eq!(parse_quick("just a title"), QuickAdd { title: "just a title".into(), ..Default::default() });
    }

    #[test]
    fn relative_format() {
        assert_eq!(fmt_rel_from(1000 + 7200, 1000), "in 2h");
        assert_eq!(fmt_rel_from(1000 - 3 * 86400, 1000), "3d ago");
        assert_eq!(fmt_rel_from(1010, 1000), "now");
    }
}
