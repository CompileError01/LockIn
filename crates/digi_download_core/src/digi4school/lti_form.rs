use crate::buffered_response::BufferedResponse;
use reqwest::{Client, Method, Url};
use scraper::{ElementRef, Html, Selector};
use std::collections::HashMap;
use std::str::FromStr;

const BAD_LTI_FORM_MESSAGE: &str = "Bad LTI-form";

/// LTI-form is just a bad redirect from digi4school
pub(crate) struct LTIForm {
    url: Url,
    method: Method,

    form_data: HashMap<String, String>,
}

impl LTIForm {
    pub async fn follow(
        resp: BufferedResponse,
        client: &Client,
    ) -> Result<BufferedResponse, reqwest::Error> {
        Ok(match LTIForm::new(&resp) {
            Some(form) => form.follow_recursively(client).await?,
            None => resp,
        })
    }

    pub fn new(raw_form: &BufferedResponse) -> Option<Self> {
        let doc = Html::parse_document(&raw_form.text());
        let selector = Selector::parse("form#lti").unwrap();

        let mut iter = doc.root_element().select(&selector);

        let html_form = iter.next()?;

        assert!(
            iter.next().is_none(),
            "{BAD_LTI_FORM_MESSAGE}: Found 2 valid HTML items for 'form#lti' css selector"
        );
        assert_eq!(
                Self::expect_form_attr(
                    html_form,
                    "enctype", // originally 'encType' but for some reason scraper converts it to lowercase
                    "encoding"
                ),
                "application/x-www-form-urlencoded",

                "{BAD_LTI_FORM_MESSAGE}: Encoding Type is not 'application/x-www-form-urlencoded' but '{}'",
                html_form.attr("encType").unwrap()
            );
        assert_eq!(
            Self::expect_form_attr(html_form, "name", ""),
            "ltiLaunchForm",
            "{BAD_LTI_FORM_MESSAGE}: Does not have `ltiLaunchForm` name, despite having an #lti id"
        );

        Some(LTIForm {
            url: Url::from_str(&Self::expect_form_attr(html_form, "action", "url")).unwrap_or_else(
                |_| panic!("{BAD_LTI_FORM_MESSAGE}: header 'action' (url) is not a URL"),
            ),

            method: Method::from_str(
                &Self::expect_form_attr(html_form, "method", "").to_uppercase(),
            )
            .unwrap(), // unwrap is fine because this can't actually fail (bad FromStr implementation)

            form_data: html_form
                .children()
                .map(|node| {
                    let element = ElementRef::wrap(node).unwrap();

                    Some((
                        Self::expect_form_attr(element, "name", ""),
                        Self::expect_form_attr(element, "value", ""),
                    ))
                })
                .collect::<Option<HashMap<String, String>>>()
                .unwrap(),
        })
    }

    pub async fn follow_recursively(
        self,
        client: &Client,
    ) -> Result<BufferedResponse, reqwest::Error> {
        let mut form = self;

        // loop because recursive async functions are cursed
        loop {
            let resp = form.send(client).await?;

            form = match LTIForm::new(&resp) {
                Some(form) => form,
                None => {
                    return Ok(resp);
                }
            }
        }
    }

    async fn send(self, client: &Client) -> Result<BufferedResponse, reqwest::Error> {
        BufferedResponse::new(
            client
                .request(self.method, self.url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(serde_urlencoded::to_string(self.form_data).unwrap())
                .send()
                .await?,
        )
        .await
    }

    fn expect_form_attr(form_html: ElementRef, attribute: &str, alias: &str) -> String {
        let panic_closure = || {
            let alias = {
                if !alias.is_empty() {
                    format!(" ({})", alias)
                } else {
                    "".to_string()
                }
            };

            panic!(
                "{BAD_LTI_FORM_MESSAGE}: form input didn't specify '{}'{}.\nHTML Element: {}",
                attribute,
                alias,
                form_html.html()
            )
        };

        form_html
            .attr(attribute)
            .unwrap_or_else(panic_closure)
            .to_string()
    }
}
