use thiserror::Error;

#[derive(Error, Debug)]
pub enum ScraperError {
    #[error(transparent)]
    PdfError(#[from] lopdf::Error),

    #[error(transparent)]
    Request(#[from] reqwest::Error),
}

#[derive(Error, Debug)]
pub enum LoginError {
    #[error("Your login information was invalid")]
    BadLogin,

    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
}

#[derive(Error, Debug)]
pub enum DigiDownloadError {
    #[error(transparent)]
    Scraper(#[from] ScraperError),

    #[error(transparent)]
    Login(#[from] LoginError),

    #[error("{0}")]
    Site(String),

    #[error(transparent)]
    Request(#[from] reqwest::Error),
}
