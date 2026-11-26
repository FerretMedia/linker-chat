//! `YouTubeProvider` against a scripted Data API.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chat_core::{ChatEvent, ConnectionState};
use providers::{YouTubeCredentials, YouTubeProvider};
use serde_json::{Value, json};
use support::{Client, FakeHttpServer, Request};

const VIDEO: &str = "jfKfPfyJRdk";

fn api_key() -> YouTubeCredentials {
    YouTubeCredentials {
        api_key: Some("key".into()),
        access_token: None,
    }
}

fn start(server: &FakeHttpServer, credentials: YouTubeCredentials) -> Client {
    let mut config = support::config();
    config.endpoints.youtube_api = server.url();
    config.youtube = credentials;
    let channel = format!("youtube:{VIDEO}").parse().unwrap();
    let provider = YouTubeProvider::new(channel, &config).with_min_poll(Duration::from_millis(20));
    Client::start(provider)
}

fn video() -> (u16, String) {
    let body = json!({"items": [{
        "snippet": {"title": "Live now", "channelId": "UC1"},
        "liveStreamingDetails": {"activeLiveChatId": "chat-1", "concurrentViewers": "42"}
    }]});
    (200, body.to_string())
}

fn page(next: Option<&str>, messages: &[&str]) -> (u16, String) {
    let items: Vec<Value> = messages
        .iter()
        .map(|text| {
            json!({
                "id": format!("id-{text}"),
                "snippet": {"type": "textMessageEvent", "publishedAt": "2026-11-25T12:00:00Z",
                            "displayMessage": text},
                "authorDetails": {"channelId": "UC2", "displayName": "fan"}
            })
        })
        .collect();
    let body = json!({"nextPageToken": next, "pollingIntervalMillis": 1, "items": items});
    (200, body.to_string())
}

fn failure(status: u16, reason: &str) -> (u16, String) {
    let body = json!({"error": {"code": status, "errors": [{"reason": reason}]}});
    (status, body.to_string())
}

fn is_video_lookup(request: &Request) -> bool {
    request.target.starts_with("GET /youtube/v3/videos")
}

fn page_token(request: &Request) -> Option<&str> {
    let (_, after) = request.target.split_once("pageToken=")?;
    after.split('&').next()
}

/// Looks the video up once, then answers polls from `pages`, repeating the last.
async fn api(pages: Vec<(u16, String)>) -> FakeHttpServer {
    let polls = AtomicUsize::new(0);
    FakeHttpServer::start(move |request| {
        if is_video_lookup(request) {
            return video();
        }
        let poll = polls.fetch_add(1, Ordering::SeqCst);
        pages[poll.min(pages.len() - 1)].clone()
    })
    .await
}

#[tokio::test]
async fn resolves_the_video_then_follows_page_tokens() {
    let server = api(vec![page(Some("t1"), &["one"]), page(Some("t2"), &["two"])]).await;
    let mut client = start(&server, api_key());
    let room = client
        .until(|event| match event {
            ChatEvent::RoomState(room) => Some(room),
            _ => None,
        })
        .await;
    assert_eq!(room.title.as_deref(), Some("Live now"));
    assert_eq!(room.viewer_count, Some(42));
    client.connected().await;
    client.message("one").await;
    client.message("two").await;

    let requests = server.requests().await;
    assert!(requests[0].target.contains(&format!("id={VIDEO}")));
    assert_eq!(requests[0].header("x-goog-api-key"), Some("key"));
    assert!(requests[1].target.contains("liveChatId=chat-1"));
    assert_eq!(page_token(&requests[1]), None);
    assert_eq!(page_token(&requests[2]), Some("t1"));
    assert!(
        requests
            .iter()
            .all(|request| !request.target.contains("key="))
    );
}

#[tokio::test]
async fn an_outage_resumes_from_the_last_page_token() {
    let server = api(vec![
        page(Some("t1"), &["one"]),
        failure(503, "backendError"),
        page(Some("t2"), &["two"]),
    ])
    .await;
    let mut client = start(&server, api_key());
    client.message("one").await;
    client
        .state(ConnectionState::Reconnecting { attempt: 1 })
        .await;
    client.connected().await;
    client.message("two").await;

    let requests = server.requests().await;
    let lookups = requests.iter().filter(|r| is_video_lookup(r)).count();
    assert_eq!(lookups, 1);
    let tokens: Vec<_> = requests[1..4].iter().map(page_token).collect();
    assert_eq!(tokens, [None, Some("t1"), Some("t1")]);
}

#[tokio::test]
async fn an_expired_page_token_starts_the_chat_over() {
    let server = api(vec![
        page(Some("t1"), &["one"]),
        failure(400, "pageTokenInvalid"),
        page(Some("t2"), &["two"]),
    ])
    .await;
    let mut client = start(&server, api_key());
    client.message("two").await;
    assert_eq!(page_token(&server.requests().await[3]), None);
}

#[tokio::test]
async fn an_exhausted_quota_stops_without_retrying() {
    let server = api(vec![failure(403, "quotaExceeded")]).await;
    let mut client = start(&server, api_key());
    assert_eq!(client.error().await, "YouTube answered quotaExceeded.");
}

#[tokio::test]
async fn a_chat_with_no_next_page_has_ended() {
    let server = api(vec![page(None, &["last words"])]).await;
    let mut client = start(&server, api_key());
    client.message("last words").await;
    assert_eq!(client.error().await, "The live chat ended.");
}

#[tokio::test]
async fn missing_credentials_say_what_to_set() {
    let server = api(vec![]).await;
    let mut client = start(&server, YouTubeCredentials::default());
    assert!(client.error().await.contains("LINKER_CHAT_YOUTUBE_API_KEY"));
    assert!(server.requests().await.is_empty());
}

#[tokio::test]
async fn sends_with_the_oauth_token() {
    let server = api(vec![page(Some("t1"), &[])]).await;
    let credentials = YouTubeCredentials {
        api_key: None,
        access_token: Some("secret".into()),
    };
    let mut client = start(&server, credentials);
    assert!(client.can_send());
    client.connected().await;
    client.say("hello");
    let sent = loop {
        let requests = server.requests().await;
        match requests.into_iter().find(|r| r.target.starts_with("POST")) {
            Some(request) => break request,
            None => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    };
    assert_eq!(
        sent.target,
        "POST /youtube/v3/liveChat/messages?part=snippet"
    );
    assert_eq!(sent.header("authorization"), Some("Bearer secret"));
    assert_eq!(
        serde_json::from_str::<Value>(&sent.body).unwrap(),
        json!({"snippet": {"liveChatId": "chat-1", "type": "textMessageEvent",
               "textMessageDetails": {"messageText": "hello"}}})
    );
}

#[tokio::test]
async fn an_api_key_alone_cannot_send() {
    let server = api(vec![page(Some("t1"), &[])]).await;
    let mut client = start(&server, api_key());
    assert!(!client.can_send());
    client.connected().await;
    client.say("hello");
    assert_eq!(
        client.system().await,
        "Not sent. Sign in to YouTube to chat here."
    );
}
