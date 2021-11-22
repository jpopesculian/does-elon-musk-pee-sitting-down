#[macro_use]
extern crate serde;
#[macro_use]
extern crate thiserror;
#[macro_use]
extern crate derive_builder;
#[macro_use]
extern crate pin_project;

pub mod api;

use api::{Api, GetTweetOptsBuilder, Tweet, TweetFields};
use chrono::{DateTime, Duration, Utc};
use futures::prelude::*;
use std::env;

const TWITTER_HANDLE: &str = "elonmusk";
const MIN_TWEETS_TO_ANALYZE: usize = 1000;
const POOPS_PER_DAY: f64 = 3.;
const TWEET_SESSION_TIMEOUT_SECS: i64 = 15 * 60;

#[derive(Clone, Debug)]
pub struct TweetSession {
    end: DateTime<Utc>,
    start: DateTime<Utc>,
    num_tweets: usize,
}

impl TweetSession {
    pub fn new(tweet: Tweet) -> Self {
        Self {
            end: tweet.created_at.unwrap(),
            start: tweet.created_at.unwrap(),
            num_tweets: 1,
        }
    }

    pub fn should_include(&self, tweet: &Tweet) -> bool {
        (self.start - tweet.created_at.unwrap()) < Duration::seconds(TWEET_SESSION_TIMEOUT_SECS)
    }

    pub fn add(&mut self, tweet: Tweet) {
        self.start = tweet.created_at.unwrap();
        self.num_tweets += 1;
    }
}

#[derive(Clone, Debug)]
pub struct PoopPeriod {
    end: DateTime<Utc>,
    start: DateTime<Utc>,
    sessions: Vec<TweetSession>,
}

impl PoopPeriod {
    pub fn new(end: DateTime<Utc>) -> Self {
        Self {
            end,
            start: end,
            sessions: Vec::new(),
        }
    }

    pub fn should_include(&self, session: &TweetSession) -> bool {
        let poop_period = Duration::seconds((24. * 60. * 60. / POOPS_PER_DAY).round() as i64);
        (self.end - session.start) < poop_period
    }

    pub fn add(&mut self, session: TweetSession) {
        self.start = session.start;
        self.sessions.push(session);
    }

    pub fn poop_session(&self) -> Option<&TweetSession> {
        self.sessions
            .iter()
            .max_by_key(|session| session.num_tweets)
    }

    pub fn num_tweets(&self) -> usize {
        self.sessions.iter().map(|session| session.num_tweets).sum()
    }

    pub fn num_poop_tweets(&self) -> usize {
        self.poop_session()
            .map(|session| session.num_tweets)
            .unwrap_or_default()
    }

    pub fn num_non_poop_tweets(&self) -> usize {
        self.num_tweets() - self.num_poop_tweets()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let token =
        env::var("TWITTER_API_BEARER_TOKEN").expect("TWITTER_API_BEARER_TOKEN should be set");
    let api = Api::new(token.into());

    let elonmusk = api
        .get_user_by_username(TWITTER_HANDLE)
        .await
        .expect("retrieving user shouldn't fail");

    let mut tweet_num = 0;
    let mut end_time = Utc::now();
    let mut poop_period: Option<PoopPeriod> = None;
    let mut tweet_session: Option<TweetSession> = None;
    let mut poop_tweets = 0;
    let mut non_poop_tweets = 0;

    while tweet_num < MIN_TWEETS_TO_ANALYZE {
        let mut tweets = api
            .get_user_tweets(
                &elonmusk.id,
                Some(
                    GetTweetOptsBuilder::default()
                        .tweet_fields([TweetFields::CreatedAt].into())
                        .max_results(Some(100))
                        .end_time(Some(end_time))
                        .build()
                        .unwrap(),
                ),
            )
            .expect("tweet request should be valid");
        while let Some(tweet) = tweets.next().await {
            let tweet = tweet.expect("retrieving tweets shouldn't fail");
            println!("{:?}", tweet);
            tweet_num += 1;
            end_time = tweet
                .created_at
                .expect("tweet should have created at")
                .into();
            if poop_period.is_none() {
                poop_period = Some(PoopPeriod::new(end_time));
            }
            let period = poop_period.as_mut().unwrap();
            if let Some(session) = tweet_session.as_mut() {
                if session.should_include(&tweet) {
                    session.add(tweet)
                } else {
                    println!("{:#?}", session);
                    if !period.should_include(session) {
                        println!("{:#?}", period);
                        poop_tweets += period.num_poop_tweets();
                        non_poop_tweets += period.num_non_poop_tweets();
                        *period = PoopPeriod::new(end_time);
                    }
                    period.add(session.clone());
                    *session = TweetSession::new(tweet);
                }
            } else {
                tweet_session = Some(TweetSession::new(tweet));
            }
        }
    }

    println!(
        "total: {}\tpoop: {}\t not poop: {}",
        tweet_num, poop_tweets, non_poop_tweets
    );
}
