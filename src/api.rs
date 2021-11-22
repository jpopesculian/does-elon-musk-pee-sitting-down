use chrono::{DateTime, Utc};
use core::fmt;
use futures::future::{ready, Fuse, FusedFuture};
use futures::prelude::*;
use parse_display::Display;
use reqwest::Client;
use serde::de::DeserializeOwned;
use std::collections::HashSet;
use std::pin::Pin;
use std::task::{Context, Poll};
use url::Url;

#[derive(Clone, Copy, Debug, Display, PartialEq, Eq, Hash)]
#[display(style = "snake_case")]
pub enum TweetFields {
    CreatedAt,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tweet {
    pub id: String,
    pub text: String,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    pub username: String,
}

#[derive(Debug, Clone, Builder)]
pub struct GetTweetOpts {
    #[builder(default)]
    tweet_fields: HashSet<TweetFields>,
    #[builder(default)]
    max_results: Option<usize>,
    #[builder(default)]
    end_time: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiMeta {
    result_count: usize,
    oldest_id: Option<String>,
    newest_id: Option<String>,
    next_token: Option<String>,
    previous_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiData<T> {
    data: Option<T>,
    meta: Option<ApiMeta>,
}

#[derive(Error, Debug)]
pub enum ApiError {
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error(transparent)]
    Url(#[from] url::ParseError),
}

pub type Result<T, E = ApiError> = core::result::Result<T, E>;

#[derive(Clone)]
pub struct BearerToken(String);

impl From<String> for BearerToken {
    fn from(token: String) -> Self {
        BearerToken(token)
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BearerToken").finish()
    }
}

impl fmt::Display for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug)]
pub struct Api {
    base_url: Url,
    client: Client,
    token: BearerToken,
}

#[pin_project]
struct ApiResult<T> {
    future: Pin<Box<dyn Future<Output = Result<ApiData<T>>>>>,
}

impl<T> ApiResult<T>
where
    T: DeserializeOwned + 'static,
{
    fn get(api: &Api, url: Url) -> Self {
        // println!(r#"http {} "Authorization:Bearer {}""#, url, api.token);
        Self {
            future: api
                .client
                .get(url)
                .bearer_auth(&api.token)
                .send()
                // TODO this can be improved upon by providing more detail from the payload
                .and_then(|res| {
                    // println!("{:#?}", res);
                    ready(res.error_for_status())
                })
                .and_then(|res| res.json::<ApiData<T>>())
                // .inspect_ok(|res| println!("{:#?}", res.meta))
                .err_into()
                .boxed(),
        }
    }

    async fn data(self) -> Result<Option<T>> {
        Ok(self.await?.data)
    }
}

impl<T> Future for ApiResult<T> {
    type Output = Result<ApiData<T>>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(self.project().future).poll(cx)
    }
}

#[pin_project]
pub struct ApiResults<T> {
    api: Api,
    url: Url,
    #[pin]
    result: Fuse<ApiResult<Vec<T>>>,
    items: std::vec::IntoIter<T>,
    pagination_token: Option<String>,
}

impl<T> ApiResults<T>
where
    T: DeserializeOwned + 'static,
{
    fn get(api: Api, url: Url) -> Self {
        let result = ApiResult::get(&api, url.clone()).fuse();
        Self {
            api,
            url,
            result,
            items: vec![].into_iter(),
            pagination_token: None,
        }
    }
}

impl<T> Stream for ApiResults<T>
where
    T: DeserializeOwned + 'static,
{
    type Item = Result<T>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let mut result = this.result;
        let url = this.url;
        let items = this.items;
        let pagination_token = this.pagination_token;

        if let Some(item) = items.next() {
            return Poll::Ready(Some(Ok(item)));
        }

        if result.is_terminated() {
            if let Some(token) = pagination_token.as_ref() {
                let mut new_url = url.clone();
                new_url
                    .query_pairs_mut()
                    .append_pair("pagination_token", token);
                *result = ApiResult::get(this.api, new_url).fuse();
            } else {
                return Poll::Ready(None);
            }
        }

        match result.poll(cx) {
            Poll::Ready(res) => match res {
                Ok(data) => {
                    *pagination_token = data.meta.and_then(|meta| meta.next_token);
                    *items = data.data.unwrap_or_default().into_iter();
                    Poll::Ready(items.next().map(Ok))
                }
                Err(err) => Poll::Ready(Some(Err(err))),
            },
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Api {
    pub fn new(token: BearerToken) -> Self {
        Self {
            base_url: Url::parse("https://api.twitter.com/").unwrap(),
            client: Client::new(),
            token,
        }
    }

    pub async fn get_user_by_username(&self, username: &str) -> Result<User> {
        let url = self
            .base_url
            .join(&format!("/2/users/by/username/{}", username))?;
        Ok(ApiResult::get(&self, url).data().await?.unwrap())
    }

    pub fn get_user_tweets(
        &self,
        user_id: &str,
        opts: Option<GetTweetOpts>,
    ) -> Result<ApiResults<Tweet>> {
        let mut url = self
            .base_url
            .join(&format!("/2/users/{}/tweets", user_id))?;
        if let Some(opts) = opts {
            let mut query = url.query_pairs_mut();
            if !opts.tweet_fields.is_empty() {
                query.append_pair(
                    "tweet.fields",
                    opts.tweet_fields
                        .iter()
                        .map(|f| f.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                        .as_str(),
                );
            }
            if let Some(max_results) = opts.max_results {
                query.append_pair("max_results", &max_results.to_string());
            }
            if let Some(end_time) = opts.end_time {
                query.append_pair(
                    "end_time",
                    &end_time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                );
            }
        }
        Ok(ApiResults::get(self.clone(), url))
    }
}
