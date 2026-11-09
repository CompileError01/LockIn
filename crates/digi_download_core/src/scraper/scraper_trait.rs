use crate::error::ScraperError;
use crate::scraper::base_scraper::BaseScraper;
use crate::scraper::util::merge_pdf;
use async_trait::async_trait;
use lopdf::Document;
use std::fmt::Debug;
use std::io::Cursor;

#[async_trait]
pub trait Scraper: BaseScraper + Sync + Send + Debug {
    async fn fetch_page_raw_pdf(&self, page: u16) -> Result<Vec<u8>, reqwest::Error>; // pdf bytes

    async fn fetch_page_pdf(&self, page: u16) -> Result<Document, ScraperError> {
        Ok(Document::load_from(Cursor::new(
            self.fetch_page_raw_pdf(page).await?,
        ))?)
    }

    async fn download_book(&self) -> Result<Document, ScraperError> {
        let page_count = self.fetch_page_count().await?;
        assert!(page_count >= 1, "no pages to download");

        let mut merge_doc = None;

        for i in 1..=page_count {
            let page = self.fetch_page_pdf(i).await?;

            match merge_doc {
                None => merge_doc = Some(page),
                Some(doc) => merge_doc = Some(merge_pdf(doc, page)?),
            }
        }

        Ok(merge_doc.unwrap())
    }
}
