use crate::buffered_response::BufferedResponse;
use crate::digi4school::lti_form::LTIForm;
use crate::digi4school::volume::Volume;
use crate::regex_builder;
use getset::{CopyGetters, Getters};
use regex::RegexBuilder;
use reqwest::{Client, Url};
use std::fmt::Display;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug, Clone, Getters, CopyGetters)]
pub struct Book {
    code: String,

    /// Keeps track of the date at which the book was obtained.
    /// Used to categorize the books in the user interface.
    #[getset(get_copy = "pub")]
    year: u16,
    #[getset(get = "pub")]
    title: String,
    #[getset(get = "pub")]
    thumbnail: Url,

    client: Arc<Client>,
}

impl Book {
    const BASE_URL: &'static str = "https://digi4school.at";

    pub(crate) fn new(
        code: String,
        year: u16,
        thumbnail: Url,
        name: &str,
        client: Arc<Client>,
    ) -> Self {
        Self {
            code,

            year,
            thumbnail,
            title: name.to_string(),

            client,
        }
    }

    pub async fn get_volumes(&self) -> Result<Vec<Volume>, reqwest::Error> {
        let resp = LTIForm::follow(
            BufferedResponse::new(self.client.get(self.base_url()).send().await?).await?,
            &self.client,
        )
        .await?;

        // If the book is loaded directly (meaning it only has one volume) it will always have a `<DOCTYPE html>` tag.
        if resp.text().trim_start().to_lowercase().starts_with("<!doctype html>") && resp.text().contains("IDRViewer") {
            let volume = Volume::from_single_volume_book(self, resp);
            Ok(vec![volume])
        } else {
            let base = resp.url().as_str().trim_end_matches('/').trim_end_matches("/index.html").to_string();
            let volumes: Vec<Volume> = regex_builder!(
                    RegexBuilder::new(
                        r#"<a( class="")? href="(.+?)" target="_blank">[\s\S]+?<img src="(.+?)" />[\s\S]+?<div class="tx"><h1>(.+?)</h1></div>"#
                    ).dot_matches_new_line(true)
                ).captures_iter(&resp.text())
                .map(|c| {
                    Volume::new(
                        Self::relative_url(&base, c.get(2).unwrap().as_str()), // URL
                        c.get(4).unwrap().into(), // name
                        Self::relative_url(&base, c.get(3).unwrap().as_str()), // thumbnail

                        self.client.clone(),
                    )
                })
                .collect();

            assert!(!volumes.is_empty(), "the book opened at a page with no pages or volumes: {base}");
            Ok(volumes)
        }
    }

    // Needed for `Volume::from_single_volume_book`
    pub(crate) fn client(&self) -> Arc<Client> {
        self.client.clone()
    }

    fn relative_url(base: &str, path: &str) -> Url {
        Url::from_str(&format!("{base}/{path}")).unwrap()
    }

    fn base_url(&self) -> String {
        format!("{}/ebook/{}", Self::BASE_URL, self.code)
    }

    /// Follows the same route as opening the book, and describes the page's structure only: tag
    /// names and addresses with their identifying parts removed. No text or attribute values.
    pub(crate) async fn debug_report(&self) -> String {
        let first = match self.client.get(self.base_url()).send().await {
            Ok(r) => r,
            Err(e) => return format!("opening a book: failed: {e}\n"),
        };
        let status = first.status();
        let buffered = match BufferedResponse::new(first).await {
            Ok(b) => b,
            Err(e) => return format!("reading a book page: failed: {e}\n"),
        };
        let mut out = format!("open status: {status}, host: {}\n", buffered.url().domain().unwrap_or("?"));
        let resp = match LTIForm::follow(buffered, &self.client).await {
            Ok(r) => r,
            Err(e) => return out + &format!("following the redirect: failed: {e}\n"),
        };
        let path: String = resp.url().path().chars().map(|c| if c.is_ascii_alphanumeric() { 'x' } else { c }).collect();
        let text = resp.text();
        out += &format!("final host: {}, path shape: {path}\n", resp.url().domain().unwrap_or("?"));
        out += &format!("has IDRViewer: {}, has makeNavBar: {}, volume links: {}\n", text.contains("IDRViewer"), text.contains("makeNavBar"), text.matches("target=\"_blank\"").count());
        let skeleton = regex::Regex::new(r"<\s*([a-zA-Z][a-zA-Z0-9-]*)").unwrap();
        let tags: Vec<&str> = skeleton.captures_iter(&text).filter_map(|c| c.get(1)).map(|m| m.as_str()).take(80).collect();
        out + &format!("first tags: {}\n", tags.join(" "))
    }
}

impl Display for Book {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("({}) {}", self.year(), self.title()))
    }
}
