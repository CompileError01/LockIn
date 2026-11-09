use crate::buffered_response::BufferedResponse;
use crate::regex;
use crate::scraper::base_scraper::BaseScraper;
use crate::scraper::scraper_trait::Scraper;
use crate::scraper::svg_scraper::SvgScraper;
use async_trait::async_trait;
use reqwest::{Client, RequestBuilder, Url};
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug)]
pub struct Digi4SchoolScraper {
    base_url: Url,
    page_count: u16,

    client: Arc<Client>,
}

impl Digi4SchoolScraper {
    pub const DOMAIN: &'static str = "a.digi4school.at";

    /// Takes first response from the `LTIForm` redirects as an input
    fn get_page_count(resp: &BufferedResponse) -> u16 {
        macro_rules! panic_closure {
            () => {|| panic!(
                "{} didn't behave as expected. Couldn't find the page number\nResponse:\n{}",
                resp.url(),
                resp.text())};
        }

        u16::from_str(
            regex!(r"IDRViewer\.makeNavBar\((\d+),'\.jpg'")
                .captures(&resp.text())
                .unwrap_or_else(panic_closure!())
                .get(1)
                .unwrap_or_else(panic_closure!())
                .as_str(),
        )
        .unwrap()
    }
}

#[async_trait]
impl SvgScraper for Digi4SchoolScraper {
    async fn get_page_raw_svg(&self, page: u16) -> Result<String, reqwest::Error> {
        assert!(
            page <= self.page_count,
            "tried downloading invalid page: {page}/{}",
            self.page_count
        );

        let url = format!("{}/{page}.svg", self.base_url);
        Ok(self.client.get(url).send().await?.text().await?)
    }

    fn get_image_request(&self, relative_url: &str) -> RequestBuilder {
        let url = format!("{}/{}", self.base_url, relative_url);
        self.client.get(url)
    }
}

#[async_trait]
impl BaseScraper for Digi4SchoolScraper {
    fn new_scraper(resp: Arc<BufferedResponse>, client: Arc<Client>) -> Box<dyn Scraper>
    where
        Self: Sized,
    {
        let base_url = resp.url().as_str().trim_end_matches("/index.html");

        Box::new(Digi4SchoolScraper {
            base_url: Url::parse(base_url).unwrap_or_else(|_| {
                panic!(
                    "Bad base_url supplied: {}.\nResponse URL: {}",
                    base_url,
                    resp.url()
                )
            }),
            page_count: Self::get_page_count(&resp),

            client,
        })
    }

    async fn fetch_page_count(&self) -> Result<u16, reqwest::Error> {
        Ok(self.page_count)
    }
}
