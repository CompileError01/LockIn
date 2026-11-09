pub use scraper_structs::get_scraper_constructor;

mod base_scraper;
mod scraper_structs;
pub(crate) mod scraper_trait;
mod svg_scraper;
mod util;

pub use util::merge_pdf;
