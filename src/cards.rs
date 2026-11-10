//! Flashcards that Claude makes in a chat, reviewed in the app with spaced repetition.

use crate::db::Db;
use crate::timeutil::{now_ts, today};
use anyhow::Result;
use chrono::{Duration, NaiveDate};
use rusqlite::params;
use std::collections::HashMap;

pub const MIN_EASE: f64 = 1.3;
pub const MAX_EASE: f64 = 2.8;
const START_EASE: f64 = 2.5;
const FRONT_CHARS: usize = 300;
const BACK_CHARS: usize = 1_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: i64,
    pub session_id: i64,
    pub front: String,
    pub back: String,
    pub due: NaiveDate,
    pub interval: i64,
    pub ease: f64,
    pub reps: i64,
}

/// How well you knew a card, from the keys 1 to 4.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rating {
    Again,
    Hard,
    Good,
    Easy,
}

impl Rating {
    pub fn from_key(c: char) -> Option<Rating> {
        match c {
            '1' => Some(Rating::Again),
            '2' => Some(Rating::Hard),
            '3' => Some(Rating::Good),
            '4' => Some(Rating::Easy),
            _ => None,
        }
    }
}

/// When the card comes back, as (due, interval in days, ease, reps). A simpler SM-2, counted in days.
pub fn schedule(card: &Card, rating: Rating, today: NaiveDate) -> (NaiveDate, i64, f64, i64) {
    let good = match card.reps {
        0 => 1,
        1 => 3,
        _ => ((card.interval.max(1) as f64) * card.ease).round() as i64,
    };
    let (interval, ease, reps) = match rating {
        Rating::Again => (0, card.ease - 0.2, 0),
        Rating::Hard => ((((card.interval.max(1)) as f64) * 1.2).round() as i64, card.ease - 0.15, card.reps + 1),
        Rating::Good => (good, card.ease, card.reps + 1),
        Rating::Easy => ((((good as f64) * 1.3).round() as i64).max(good + 1), card.ease + 0.15, card.reps + 1),
    };
    let interval = if rating == Rating::Again { 0 } else { interval.max(1) };
    (today + Duration::days(interval), interval, ease.clamp(MIN_EASE, MAX_EASE), reps)
}

fn day(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn card_row(r: &rusqlite::Row) -> rusqlite::Result<Card> {
    let due: String = r.get(4)?;
    Ok(Card {
        id: r.get(0)?,
        session_id: r.get(1)?,
        front: r.get(2)?,
        back: r.get(3)?,
        due: NaiveDate::parse_from_str(&due, "%Y-%m-%d").unwrap_or_else(|_| today()),
        interval: r.get(5)?,
        ease: r.get(6)?,
        reps: r.get(7)?,
    })
}

const COLS: &str = "id,session_id,front,back,due,interval_days,ease,reps";

/// Adds new cards, due today, and skips any whose front is already in this chat's deck.
pub fn add_cards(db: &Db, session_id: i64, cards: &[(String, String)]) -> Result<usize> {
    let mut added = 0;
    for (front, back) in cards {
        let front: String = front.trim().chars().take(FRONT_CHARS).collect();
        let back: String = back.trim().chars().take(BACK_CHARS).collect();
        if front.is_empty() || back.is_empty() {
            continue;
        }
        added += db.c.execute(
            "INSERT INTO cards(session_id,front,back,due,interval_days,ease,reps,created_at)
             SELECT ?1,?2,?3,?4,0,?5,0,?6 WHERE NOT EXISTS (SELECT 1 FROM cards WHERE session_id=?1 AND front=?2)",
            params![session_id, front, back, day(today()), START_EASE, now_ts()],
        )?;
    }
    Ok(added)
}

/// This chat's cards, the ones due today first when `all` is off and only those.
pub fn cards(db: &Db, session_id: i64, all: bool) -> Result<Vec<Card>> {
    let filter = if all { "" } else { " AND due<=?2" };
    let mut st = db.c.prepare(&format!("SELECT {COLS} FROM cards WHERE session_id=?1{filter} ORDER BY due,id"))?;
    let rows = if all {
        st.query_map([session_id], card_row)?.collect::<rusqlite::Result<_>>()?
    } else {
        st.query_map(params![session_id, day(today())], card_row)?.collect::<rusqlite::Result<_>>()?
    };
    Ok(rows)
}

pub fn rate(db: &Db, card: &Card, rating: Rating) -> Result<Card> {
    let (due, interval, ease, reps) = schedule(card, rating, today());
    db.c.execute("UPDATE cards SET due=?2,interval_days=?3,ease=?4,reps=?5 WHERE id=?1", params![card.id, day(due), interval, ease, reps])?;
    Ok(Card { due, interval, ease, reps, ..card.clone() })
}

pub fn delete_card(db: &Db, id: i64) -> Result<()> {
    db.c.execute("DELETE FROM cards WHERE id=?1", [id])?;
    Ok(())
}

pub fn delete_deck(db: &Db, session_id: i64) -> Result<()> {
    db.c.execute("DELETE FROM cards WHERE session_id=?1", [session_id])?;
    Ok(())
}

/// For each chat with cards: how many are due today, and how many there are.
pub fn counts(db: &Db) -> Result<HashMap<i64, (usize, usize)>> {
    let mut st = db.c.prepare("SELECT session_id, sum(due<=?1), count(*) FROM cards GROUP BY session_id")?;
    let rows = st.query_map([day(today())], |r| Ok((r.get::<_, i64>(0)?, (r.get::<_, i64>(1)? as usize, r.get::<_, i64>(2)? as usize))))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The day the next card in this chat comes back, if any are waiting.
pub fn next_due(db: &Db, session_id: i64) -> Result<Option<NaiveDate>> {
    let d: Option<String> = db.c.query_row("SELECT min(due) FROM cards WHERE session_id=?1", [session_id], |r| r.get(0))?;
    Ok(d.and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(reps: i64, interval: i64, ease: f64) -> Card {
        Card { id: 1, session_id: 1, front: "f".into(), back: "b".into(), due: today(), interval, ease, reps }
    }

    #[test]
    fn good_answers_space_cards_out_and_again_brings_them_back_today() {
        let t = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        let (due, i, _, r) = schedule(&card(0, 0, 2.5), Rating::Good, t);
        assert_eq!((due, i, r), (t + Duration::days(1), 1, 1));
        let (_, i, _, _) = schedule(&card(1, 1, 2.5), Rating::Good, t);
        assert_eq!(i, 3);
        let (_, i, _, _) = schedule(&card(2, 3, 2.5), Rating::Good, t);
        assert_eq!(i, 8);
        let (due, i, ease, r) = schedule(&card(4, 20, 2.5), Rating::Again, t);
        assert_eq!((due, i, r), (t, 0, 0));
        assert!((ease - 2.3).abs() < 1e-9);
    }

    #[test]
    fn easy_beats_good_and_ease_stays_in_bounds() {
        let t = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        let good = schedule(&card(0, 0, 2.5), Rating::Good, t).1;
        let easy = schedule(&card(0, 0, 2.5), Rating::Easy, t).1;
        assert!(easy > good);
        assert_eq!(schedule(&card(3, 5, MAX_EASE), Rating::Easy, t).2, MAX_EASE);
        assert_eq!(schedule(&card(3, 5, MIN_EASE), Rating::Again, t).2, MIN_EASE);
        assert_eq!(schedule(&card(3, 5, MIN_EASE), Rating::Hard, t).1, 6);
    }

    #[test]
    fn decks_skip_duplicates_and_only_due_cards_are_reviewed() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch(crate::db::SCHEMA).unwrap();
        let db = Db { c };
        let id = crate::tutor::create_chat(&db, "Bio").unwrap();
        let pair = |f: &str, b: &str| (f.to_string(), b.to_string());
        assert_eq!(add_cards(&db, id, &[pair("Cell?", "Unit of life"), pair("Cell?", "dup"), pair(" ", "x")]).unwrap(), 1);
        assert_eq!(add_cards(&db, id, &[pair("DNA?", "Genes")]).unwrap(), 1);
        let due = cards(&db, id, false).unwrap();
        assert_eq!(due.len(), 2);
        rate(&db, &due[0], Rating::Good).unwrap();
        assert_eq!(cards(&db, id, false).unwrap().len(), 1);
        assert_eq!(cards(&db, id, true).unwrap().len(), 2);
        assert_eq!(counts(&db).unwrap()[&id], (1, 2));
        crate::tutor::delete_session(&db, id).unwrap();
        assert!(cards(&db, id, true).unwrap().is_empty());
    }
}
