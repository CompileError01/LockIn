use crate::buffered_response::BufferedResponse;
use crate::scraper::base_scraper::BaseScraper;
use crate::scraper::scraper_trait::Scraper;
use async_trait::async_trait;
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use regex::Regex;
use reqwest::RequestBuilder;
use std::collections::HashMap;
use std::fmt::Debug;

#[async_trait]
pub trait SvgScraper: BaseScraper + Sync + Send + Debug {
    /// get an unmodified svg directly from the page
    async fn get_page_raw_svg(&self, page: u16) -> Result<String, reqwest::Error>;
    fn get_image_request(&self, relative_url: &str) -> RequestBuilder;

    async fn get_page_svg(&self, page: u16) -> Result<String, reqwest::Error> {
        let raw_svg = self.get_page_raw_svg(page).await?;
        let mut svg = raw_svg.clone();

        let mut images = HashMap::new();

        let url_regex = Regex::new(&format!("xlink:href=\"({}/.+?)\"/>", page)).unwrap();
        // TODO add rayon iter
        for capture in url_regex.captures_iter(&raw_svg) {
            let url = capture.get(1).unwrap().as_str();

            // skip already downloaded images
            if !images.contains_key(url) {
                let resp = BufferedResponse::new(self.get_image_request(url).send().await?).await?;
                images.insert(url, resp);
            };
        }

        for (url, resp) in images {
            let content_type = {
                let content_type_header = resp.headers().get("Content-Type").unwrap_or_else(|| {
                    panic!(
                        "No Content-Type specified for downloaded content: {}",
                        resp.url().as_str()
                    )
                });

                content_type_header
                    .to_str()
                    .unwrap_or_else(|_| {
                        panic!(
                            "Content-Type is not a valid string: {:?}",
                            content_type_header
                        )
                    })
                    .to_owned()
            };

            svg = svg.replace(
                url,
                &format!(
                    "data:{};base64,{}",
                    content_type,
                    &BASE64_STANDARD.encode(resp.bytes())
                ),
            );
        }

        Ok(svg)
    }

    async fn fetch_page_pdf(&self, page: u16) -> Result<Vec<u8>, reqwest::Error> {
        let svg = self.get_page_svg(page).await?;
        let tree =
            svg2pdf::usvg::Tree::from_str(&svg, &Default::default()).expect("malformed svg found");

        Ok(
            svg2pdf::to_pdf(&tree, Default::default(), Default::default())
                .expect("failed to convert svg to pdf"),
        )
    }
}

#[async_trait]
impl<T> Scraper for T
where
    T: SvgScraper,
{
    async fn fetch_page_raw_pdf(&self, page: u16) -> Result<Vec<u8>, reqwest::Error> {
        SvgScraper::fetch_page_pdf(self, page).await
    }
}
