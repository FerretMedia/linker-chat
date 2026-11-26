//! `KickProvider` against a scripted Pusher socket and Kick site.

mod support;

use chat_core::{ChatEvent, ConnectionState};
use providers::KickProvider;
use serde_json::{Value, json};
use support::{Client, FakeHttpServer, FakeSocket, FakeSocketServer};

async fn kick_site() -> FakeHttpServer {
    FakeHttpServer::start(|request| match request.target.as_str() {
        "GET /api/v2/channels/some-one" => (
            200,
            json!({
                "id": 5, "user_id": 77, "slug": "some-one",
                "chatroom": { "id": 1234 },
                "livestream": { "session_title": "Live now", "viewer_count": 42 }
            })
            .to_string(),
        ),
        "GET /api/v2/channels/blocked" => (403, "{}".into()),
        "POST /api/v1/realtime/connection" => (
            200,
            json!({"data": {"mode": "websocket", "connections": [
                {"provider": "centrifugo", "credentials": {"url": "wss://elsewhere"}},
                {"provider": "pusher", "credentials": {"app_key": "key-1", "cluster": "us2"}}
            ]}})
            .to_string(),
        ),
        "POST /public/v1/chat" => (200, "{}".into()),
        _ => (404, "{}".into()),
    })
    .await
}

fn provider(
    channel: &str,
    site: &FakeHttpServer,
    socket: &FakeSocketServer,
    token: Option<&str>,
) -> KickProvider {
    let mut config = support::config();
    config.endpoints.kick_site = site.url();
    config.endpoints.kick_api = site.url();
    config.endpoints.kick_realtime = site.url();
    config.endpoints.kick_pusher = socket.url();
    config.kick_token = token.map(Into::into);
    KickProvider::new(format!("kick:{channel}").parse().unwrap(), &config)
}

async fn subscribed(server: &FakeSocketServer) -> FakeSocket {
    let mut socket = server.accept().await;
    socket
        .send(r#"{"event":"pusher:connection_established","data":"{\"socket_id\":\"1.2\"}"}"#)
        .await;
    let request: Value = serde_json::from_str(&socket.receive().await).unwrap();
    assert_eq!(
        request,
        json!({"event": "pusher:subscribe", "data": {"auth": "", "channel": "chatrooms.1234.v2"}})
    );
    socket
        .send(r#"{"event":"pusher_internal:subscription_succeeded","channel":"chatrooms.1234.v2","data":"{}"}"#)
        .await;
    socket
}

fn chat_event(id: &str, content: &str) -> String {
    let message = json!({
        "id": id, "content": content, "type": "message",
        "created_at": "2026-11-25T12:00:00+00:00",
        "sender": { "id": 1, "username": "Fan", "slug": "fan",
                    "identity": { "color": "#75FD46", "badges": [] } }
    });
    json!({
        "event": "App\\Events\\ChatMessageEvent",
        "channel": "chatrooms.1234.v2",
        "data": message.to_string(),
    })
    .to_string()
}

#[tokio::test]
async fn looks_up_the_chatroom_subscribes_and_reads() {
    let (site, pusher) = (kick_site().await, FakeSocketServer::start().await);
    let mut client = Client::start(provider("some-one", &site, &pusher, None));
    let mut socket = subscribed(&pusher).await;

    let room = client
        .until(|event| match event {
            ChatEvent::RoomState(room) => Some(room),
            _ => None,
        })
        .await;
    assert_eq!(room.viewer_count, Some(42));
    assert_eq!(room.title.as_deref(), Some("Live now"));
    client.connected().await;

    socket.send(&chat_event("m1", "hi [emote:1:KEKW]")).await;
    client.message("hi KEKW").await;

    socket.send(r#"{"event":"pusher:ping","data":{}}"#).await;
    socket
        .receive_where(|text| text.contains("pusher:pong"))
        .await;

    client.say("hi");
    assert_eq!(
        client.system().await,
        "Not sent. Sign in to Kick to chat here."
    );
}

#[tokio::test]
async fn a_dropped_socket_resubscribes_without_another_lookup() {
    let (site, pusher) = (kick_site().await, FakeSocketServer::start().await);
    let mut client = Client::start(provider("some-one", &site, &pusher, None));
    let socket = subscribed(&pusher).await;
    client.connected().await;
    drop(socket);
    client
        .state(ConnectionState::Reconnecting { attempt: 1 })
        .await;
    let mut socket = subscribed(&pusher).await;
    client.connected().await;
    socket.send(&chat_event("m2", "back")).await;
    client.message("back").await;
    let lookups = |target: &'static str| {
        let site = &site;
        async move {
            let requests = site.requests().await;
            requests
                .iter()
                .filter(|r| r.target.starts_with(target))
                .count()
        }
    };
    assert_eq!(lookups("GET /api/v2/channels").await, 1);
    assert_eq!(lookups("POST /api/v1/realtime").await, 2);
}

#[tokio::test]
async fn sends_through_the_official_api() {
    let (site, pusher) = (kick_site().await, FakeSocketServer::start().await);
    let mut client = Client::start(provider("some-one", &site, &pusher, Some("secret")));
    assert!(client.can_send());
    let _socket = subscribed(&pusher).await;
    client.connected().await;

    client.reply("hello", Some("parent"));
    let sent = loop {
        let requests = site.requests().await;
        match requests
            .into_iter()
            .find(|r| r.target == "POST /public/v1/chat")
        {
            Some(request) => break request,
            None => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
        }
    };
    assert_eq!(sent.header("authorization"), Some("Bearer secret"));
    assert_eq!(
        serde_json::from_str::<Value>(&sent.body).unwrap(),
        json!({"broadcaster_user_id": 77, "content": "hello", "type": "user",
               "reply_to_message_id": "parent"})
    );
}

#[tokio::test]
async fn a_missing_or_refused_channel_stops_without_retrying() {
    let (site, pusher) = (kick_site().await, FakeSocketServer::start().await);
    let mut missing = Client::start(provider("nobody", &site, &pusher, None));
    assert_eq!(
        missing.error().await,
        "There is no Kick channel named nobody."
    );
    let mut blocked = Client::start(provider("blocked", &site, &pusher, None));
    assert!(blocked.error().await.starts_with("Kick refused"));
}

#[tokio::test]
async fn a_numeric_channel_is_a_chatroom_id() {
    let (site, pusher) = (kick_site().await, FakeSocketServer::start().await);
    let mut client = Client::start(provider("1234", &site, &pusher, None));
    let _socket = subscribed(&pusher).await;
    client.connected().await;
    let requests = site.requests().await;
    assert!(requests.iter().all(|r| !r.target.starts_with("GET")));
}
