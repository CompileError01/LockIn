use reqwest::Response;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};

/// Stores a response and keeps track of its body in one type.
/// Circumvents consumption of Response by `.text()` or `.bytes()`
/// Does not lazily load the body -> Use only use if you are sure you need the text
#[derive(Debug)]
pub struct BufferedResponse {
    resp: Response,
    buf: Vec<u8>,

    utf8_check_passed: AtomicBool,
}

impl BufferedResponse {
    pub async fn new(mut resp: Response) -> Result<Self, reqwest::Error> {
        let buf = Self::get_buf(&mut resp).await?;

        Ok(Self {
            resp,
            buf,

            utf8_check_passed: AtomicBool::new(false),
        })
    }

    pub fn text(&self) -> String {
        if self.utf8_check_passed.load(Ordering::Acquire) {
            unsafe { return String::from_utf8_unchecked(self.buf.clone()) }
        }

        let result = String::from_utf8(self.buf.clone()).expect("expected charset to be utf8");
        self.utf8_check_passed.store(true, Ordering::Release);
        result
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }

    async fn get_buf(resp: &mut Response) -> Result<Vec<u8>, reqwest::Error> {
        let content_length = resp.content_length();

        let mut buf: Vec<u8>;
        match content_length {
            Some(length) => {
                buf = Vec::with_capacity(length.try_into().unwrap());
            }
            None => buf = Vec::new(),
        }

        loop {
            if let Some(chunk) = resp.chunk().await? {
                buf.append(&mut chunk.to_vec());
            } else {
                buf.shrink_to_fit();
                return Ok(buf);
            };
        }
    }
}

impl Deref for BufferedResponse {
    type Target = Response;

    fn deref(&self) -> &Self::Target {
        &self.resp
    }
}
