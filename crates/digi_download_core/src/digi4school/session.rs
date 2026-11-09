use crate::digi4school::book::Book;
use crate::error::{DigiDownloadError, LoginError};
use chrono::NaiveDate;
use reqwest::{Client, Url};
use serde::Serialize;
use serde_json::Value;
use std::str::FromStr;
use std::sync::Arc;

pub struct Session {
    client: Arc<Client>,
}

#[derive(Serialize)]
struct LoginData {
    email: String,
    password: String,
    indefinite: u8,
}

impl Session {
    pub(crate) const BASE_URL: &'static str = "https://digi4school.at";

    pub async fn new(email: String, password: String) -> Result<Self, LoginError> {
        let builder = Client::builder().cookie_store(true);

        let session = Self {
            client: Arc::new(builder.build().unwrap()),
        };
        session.login(email, password, false).await?;

        Ok(session)
    }

    /// Your books, from the JSON API the digi4school web app uses. Expired books are left out,
    /// because the site no longer opens them.
    pub async fn get_books(&self) -> Result<Vec<Book>, DigiDownloadError> {
        let data = self.synch().await?;
        Ok(parse_books(&data, chrono::Local::now().date_naive())
            .into_iter()
            .map(|b| {
                let thumbnail = Url::from_str(Self::BASE_URL).expect("base URL is valid");
                Book::new(b.code, b.year, thumbnail, &b.title, self.client.clone())
            })
            .collect())
    }

    async fn synch(&self) -> Result<Value, DigiDownloadError> {
        let resp = self
            .client
            .get(format!("{}/br/xhr/v2/synch", Self::BASE_URL))
            .header("Accept", "application/json")
            .send()
            .await?;
        // The web app treats 260 as an expired or missing login.
        if resp.status().as_u16() == 260 {
            return Err(DigiDownloadError::Site("the site didn't accept the login for this request".into()));
        }
        let status = resp.status();
        let text = resp.text().await?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|_| DigiDownloadError::Site(format!("the book list wasn't JSON (HTTP {status})")))?;
        if let Some(message) = value.get("msg").and_then(Value::as_str) {
            return Err(DigiDownloadError::Site(message.to_string()));
        }
        Ok(value)
    }

    /// Describes what the site sends back, without names, emails, titles or other values, so the
    /// result is safe to share when the site changes and needs fixing.
    pub async fn debug_report(&self) -> String {
        let mut out = String::new();
        let data = match self.synch().await {
            Ok(d) => d,
            Err(e) => return format!("book list: failed: {e}\n"),
        };
        let fields = |v: &Value| v.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_else(|| "(not an object)".into());
        out += &format!("top-level fields: {}\n", fields(&data));
        let list = books_array(&data);
        out += &format!("books in response: {}\n", list.len());
        if let Some(first) = list.first() {
            out += &format!("book fields: {}\n", fields(first));
        }
        out += &format!("books usable after parsing: {}\n", parse_books(&data, chrono::Local::now().date_naive()).len());
        match self.get_books().await.ok().and_then(|b| b.into_iter().next()) {
            Some(book) => out += &book.debug_report().await,
            None => out += "no usable book to open\n",
        }
        out
    }

    pub fn redeem_code(&self) -> bool {
        todo!("Program once I can test redeeming a code")
    }

    async fn login(
        &self,
        email: String,
        password: String,
        remember_login: bool,
    ) -> Result<(), LoginError> {
        let resp_content = self
            .client
            .post(format!("{}/br/xhr/login", Self::BASE_URL))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(
                serde_urlencoded::to_string(LoginData {
                    email,
                    password,
                    indefinite: u8::from(remember_login),
                })
                .unwrap(),
            )
            .send()
            .await?
            .text()
            .await?;

        match resp_content.as_str() {
            "OK" => Ok(()),
            "KO" => Err(LoginError::BadLogin),
            _ => panic!("Bad login-form response: {}", resp_content),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct BookInfo {
    pub code: String,
    pub title: String,
    pub year: u16,
}

fn books_array(data: &Value) -> Vec<Value> {
    data.get("books")
        .or_else(|| data.get("data").and_then(|d| d.get("books")))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Expiry dates look like `31.10.2031`.
fn parse_expiry(text: &str) -> Option<NaiveDate> {
    let mut parts = text.trim().split('.');
    let (d, m, y) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    NaiveDate::from_ymd_opt(y, m, d)
}

/// Reads the book list. A book without a code can't be opened, so it's skipped.
pub fn parse_books(data: &Value, today: NaiveDate) -> Vec<BookInfo> {
    books_array(data)
        .iter()
        .filter_map(|b| {
            let code = match b.get("code")? {
                Value::String(s) if !s.is_empty() => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            let expiry = b.get("expiry").and_then(Value::as_str).and_then(parse_expiry);
            if expiry.is_some_and(|d| d < today) {
                return None;
            }
            let title = b.get("title").or_else(|| b.get("name")).and_then(Value::as_str).unwrap_or(&code).trim().to_string();
            // Licences run for six years, so this gives the year the book was obtained.
            let year = expiry.map_or(0, |d| (chrono::Datelike::year(&d) - 6).max(0) as u16);
            Some(BookInfo { code, title, year })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 9).unwrap()
    }

    #[test]
    fn books_come_from_the_json_list() {
        let data = json!({"user": {}, "books": [
            {"code": "ABC123", "title": " Mathe 2 ", "expiry": "31.10.2031"},
            {"code": 4711, "title": "Physik", "expiry": "31.10.2030"},
            {"code": "NOTITLE", "expiry": "1.2.2027"},
        ]});
        let books = parse_books(&data, today());
        assert_eq!(books[0], BookInfo { code: "ABC123".into(), title: "Mathe 2".into(), year: 2025 });
        assert_eq!((books[1].code.as_str(), books[1].year), ("4711", 2024));
        assert_eq!(books[2].title, "NOTITLE");
    }

    #[test]
    fn expired_and_unusable_books_are_skipped_and_odd_data_is_survivable() {
        let data = json!({"books": [
            {"code": "OLD", "title": "Old", "expiry": "31.10.2020"},
            {"code": "", "title": "Empty code"},
            {"title": "No code"},
            {"code": "ODD", "title": "Odd date", "expiry": "soon"},
        ]});
        let books = parse_books(&data, today());
        assert_eq!(books.len(), 1);
        assert_eq!((books[0].code.as_str(), books[0].year), ("ODD", 0), "an unreadable date doesn't hide a book");
        assert!(parse_books(&json!({}), today()).is_empty());
        assert!(parse_books(&json!({"books": "nope"}), today()).is_empty());
        assert_eq!(parse_books(&json!({"data": {"books": [{"code": "N", "title": "Nested"}]}}), today()).len(), 1);
    }
}
