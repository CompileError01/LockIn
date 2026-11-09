use crate::buffered_response::BufferedResponse;
use crate::scraper::scraper_trait::Scraper;
use async_trait::async_trait;
use reqwest::Client;
use std::sync::Arc;

#[async_trait]
pub trait BaseScraper {
    fn new_scraper(resp: Arc<BufferedResponse>, client: Arc<Client>) -> Box<dyn Scraper>
    where
        Self: Sized;

    async fn fetch_page_count(&self) -> Result<u16, reqwest::Error>;
}
