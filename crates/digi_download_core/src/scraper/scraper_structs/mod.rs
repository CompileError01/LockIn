use crate::buffered_response::BufferedResponse;
use crate::scraper::base_scraper::BaseScraper;
use crate::scraper::scraper_structs::digi4school::Digi4SchoolScraper;
use crate::scraper::scraper_trait::Scraper;
use reqwest::{Client, Url};
use std::sync::Arc;

pub mod digi4school;

pub fn get_scraper_constructor(
    url: &Url,
) -> fn(Arc<BufferedResponse>, Arc<Client>) -> Box<dyn Scraper> {
    match url.domain().unwrap_or_else(|| panic!("Bad URL supplied: {url}")) {
        Digi4SchoolScraper::DOMAIN | "a.hpthek.at" => Digi4SchoolScraper::new_scraper,
        _ => unimplemented!("Scraper for '{url}'\n\
        Please open a github issue with the book you tried downloading and with the url in this error message.")
        // TODO add github issue template and insert link to open new 'Scraper not implemented' issue
    }
}
