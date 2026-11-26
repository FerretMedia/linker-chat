//! `TwitchProvider` against a scripted IRC server.

mod support;

use chat_core::{ChatEvent, ConnectionState, Role, RoomState};
use providers::{TwitchAuth, TwitchProvider};
use support::{Client, FakeSocket, FakeSocketServer};

fn provider(server: &FakeSocketServer, auth: TwitchAuth) -> TwitchProvider {
    let mut config = support::config();
    config.endpoints.twitch_irc = server.url();
    config.twitch = auth;
    TwitchProvider::new("twitch:chan".parse().unwrap(), &config)
}

fn signed_in() -> TwitchAuth {
    TwitchAuth::OAuth {
        login: "me".into(),
        token: "secret".into(),
    }
}

async fn join(server: &FakeSocketServer, login: &str) -> FakeSocket {
    let mut socket = server.accept().await;
    socket.receive_where(|line| line.starts_with("NICK")).await;
    socket
        .send(&format!(":tmi.twitch.tv 001 {login} :Welcome, GLHF!"))
        .await;
    socket.receive_where(|line| line == "JOIN #chan").await;
    socket
        .send("@room-id=1;slow=30 :tmi.twitch.tv ROOMSTATE #chan\r\n")
        .await;
    socket
}

#[tokio::test]
async fn anonymous_sessions_ask_for_tags_join_and_read() {
    let server = FakeSocketServer::start().await;
    let mut client = Client::start(provider(&server, TwitchAuth::Anonymous));
    assert!(!client.can_send());

    let mut socket = server.accept().await;
    assert_eq!(
        socket.receive().await,
        "CAP REQ :twitch.tv/tags twitch.tv/commands"
    );
    let nick = socket.receive_where(|line| line.starts_with("NICK")).await;
    assert!(nick.starts_with("NICK justinfan"), "{nick}");
    socket
        .send(":tmi.twitch.tv 001 justinfan1 :Welcome, GLHF!")
        .await;
    socket.receive_where(|line| line == "JOIN #chan").await;
    socket
        .send(
            "@room-id=1;slow=30 :tmi.twitch.tv ROOMSTATE #chan\r\n\
             @id=1 :a!a@a.tmi.twitch.tv PRIVMSG #chan :two lines in one frame\r\n",
        )
        .await;

    client.connected().await;
    client
        .until(|event| match event {
            ChatEvent::RoomState(RoomState { slow_mode_secs, .. }) => slow_mode_secs,
            _ => None,
        })
        .await;
    client.message("two lines in one frame").await;

    client.say("hi");
    assert_eq!(client.system().await, "Sign in to Twitch to chat here.");
}

#[tokio::test]
async fn pings_are_answered_with_their_token() {
    let server = FakeSocketServer::start().await;
    let _client = Client::start(provider(&server, TwitchAuth::Anonymous));
    let mut socket = join(&server, "justinfan1").await;
    socket.send("PING :tmi.twitch.tv").await;
    socket
        .receive_where(|line| line == "PONG :tmi.twitch.tv")
        .await;
}

#[tokio::test]
async fn a_sent_message_is_echoed_once_twitch_accepts_it() {
    let server = FakeSocketServer::start().await;
    let mut client = Client::start(provider(&server, signed_in()));
    assert!(client.can_send());
    let mut socket = join(&server, "me").await;
    client.connected().await;

    client.reply("hello", Some("parent"));
    socket
        .receive_where(|line| line == "@reply-parent-msg-id=parent PRIVMSG #chan :hello")
        .await;
    socket
        .send(
            "@badges=broadcaster/1;color=#1E90FF;display-name=Me;id=sent-1;mod=0 \
             :tmi.twitch.tv USERSTATE #chan",
        )
        .await;
    let echoed = client.message("hello").await;
    assert_eq!(echoed.id, "sent-1");
    assert_eq!(echoed.author.username, "me");
    assert_eq!(echoed.author.display_name, "Me");
    assert_eq!(echoed.author.role, Role::Broadcaster);
    assert_eq!(echoed.reply.unwrap().message_id, "parent");
}

#[tokio::test]
async fn a_refused_message_is_reported_and_not_echoed() {
    let server = FakeSocketServer::start().await;
    let mut client = Client::start(provider(&server, signed_in()));
    let mut socket = join(&server, "me").await;
    client.connected().await;

    client.say("too fast");
    socket.receive_where(|line| line.contains("too fast")).await;
    socket
        .send("@msg-id=msg_slowmode :tmi.twitch.tv NOTICE #chan :This room is in slow mode.")
        .await;
    assert_eq!(client.system().await, "This room is in slow mode.");

    client.say("second");
    socket.receive_where(|line| line.contains("second")).await;
    socket
        .send("@id=sent-2 :tmi.twitch.tv USERSTATE #chan")
        .await;
    let first = client
        .until(|event| match event {
            ChatEvent::Message(message) => Some(message.content),
            _ => None,
        })
        .await;
    assert_eq!(first, "second");
}

#[tokio::test]
async fn a_dropped_connection_joins_again() {
    let server = FakeSocketServer::start().await;
    let mut client = Client::start(provider(&server, TwitchAuth::Anonymous));
    let mut socket = join(&server, "justinfan1").await;
    client.connected().await;
    socket.send(":tmi.twitch.tv RECONNECT").await;
    client
        .state(ConnectionState::Reconnecting { attempt: 1 })
        .await;

    let mut socket = join(&server, "justinfan1").await;
    client.connected().await;
    socket
        .send("@id=9 :a!a@a.tmi.twitch.tv PRIVMSG #chan :back")
        .await;
    client.message("back").await;
}

#[tokio::test]
async fn a_bad_token_stops_without_retrying() {
    let server = FakeSocketServer::start().await;
    let mut client = Client::start(provider(&server, signed_in()));
    let mut socket = server.accept().await;
    socket
        .receive_where(|line| line == "PASS oauth:secret")
        .await;
    socket
        .send(":tmi.twitch.tv NOTICE * :Login authentication failed")
        .await;
    assert_eq!(
        client.error().await,
        "Twitch sign-in failed: Login authentication failed"
    );
}
