//! `NativeProvider` against the real Go server.

mod support;

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use chat_core::{
    ChatEvent, ChatStore, ConnectionState, Identity, MessageFragment, ModAction, Moderation,
    PaneMode, Removal, Role, UserModeration, UserRef, ViewState,
};
use providers::{NativeAuth, NativeProvider, ProviderCommand};
use support::Client;
use time::OffsetDateTime;

const ADMIN_TOKEN: &str = "test-admin-token";
/// HS256 with the secret `test-secret`, for user `clerk_fan`.
const CLERK_JWT: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyXzEiLCJ1c2VybmFtZSI6ImNsZXJrX2ZhbiIsIm5hbWUiOiJDbGVyayBGYW4ifQ.Q98YxfmxYHJsBCV8lL5VotzYLOOay9QXxPW5r65I0vI";

struct Server {
    process: Child,
    addr: SocketAddr,
    data: tempfile::TempDir,
}

impl Server {
    async fn start() -> Self {
        // Bind to learn a free port, then let the server take it.
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = free.local_addr().unwrap();
        drop(free);
        let data = tempfile::tempdir().unwrap();
        let process = launch(addr, &data);
        let server = Self {
            process,
            addr,
            data,
        };
        server.wait_until_ready().await;
        server
    }

    async fn wait_until_ready(&self) {
        for _ in 0..200 {
            if tokio::net::TcpStream::connect(self.addr).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the server never listened on {}", self.addr);
    }

    async fn restart(&mut self) {
        self.process.kill().unwrap();
        self.process.wait().unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.process = launch(self.addr, &self.data);
        self.wait_until_ready().await;
    }

    fn client(&self, room: &str, auth: NativeAuth) -> Client {
        let mut config = support::config();
        config.native.server_url = format!("ws://{}/ws", self.addr);
        config.native.auth = auth;
        let channel = format!("native:{room}").parse().unwrap();
        Client::start(NativeProvider::new(channel, &config))
    }

    async fn broadcaster(&self, room: &str) -> Client {
        self.joined(room, admin(), Role::Broadcaster).await
    }

    async fn guest(&self, room: &str, nickname: &str) -> Client {
        self.joined(room, guest(nickname), Role::Guest).await
    }

    async fn joined(&self, room: &str, auth: NativeAuth, role: Role) -> Client {
        let mut client = self.client(room, auth);
        client.connected().await;
        assert_eq!(identity(&mut client).await.role, role);
        client
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn launch(addr: SocketAddr, data: &tempfile::TempDir) -> Child {
    let binary = std::env::var_os("LINKER_CHAT_SERVER_BIN").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../server/bin/linker-chat-server"),
        PathBuf::from,
    );
    Command::new(&binary)
        .env_clear()
        .env("LINKER_CHAT_ADDR", addr.to_string())
        .env("LINKER_CHAT_DB", data.path().join("chat.db"))
        .env("LINKER_CHAT_ADMIN_TOKEN", ADMIN_TOKEN)
        .env("LINKER_CHAT_JWT_SECRET", "test-secret")
        .env("LINKER_CHAT_GUEST_SECRET", "test-guest-secret")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|error| {
            panic!(
                "cannot run {}: {error}. Build it in server/ with \
                 `go build -o bin/linker-chat-server ./cmd/linker-chat-server`.",
                binary.display()
            )
        })
}

fn admin() -> NativeAuth {
    NativeAuth::Token {
        token: ADMIN_TOKEN.into(),
        display_name: Some("Caster".into()),
    }
}

fn guest(nickname: &str) -> NativeAuth {
    NativeAuth::Guest {
        nickname: nickname.into(),
    }
}

async fn identity(client: &mut Client) -> Identity {
    client
        .until(|event| match event {
            ChatEvent::Identity(identity) => Some(identity),
            _ => None,
        })
        .await
}

async fn moderated(client: &mut Client) -> UserModeration {
    client
        .until(|event| match event {
            ChatEvent::UserModerated(moderation) => Some(moderation),
            _ => None,
        })
        .await
}

#[tokio::test]
async fn messages_and_replies_reach_everyone_in_the_room() {
    let server = Server::start().await;
    let mut caster = server.broadcaster("main").await;
    let mut ferret = server.guest("main", "ferret42").await;
    let mut elsewhere = server.guest("other", "otter").await;

    ferret.say("hello from a guest");
    let seen = caster.message("hello from a guest").await;
    assert_eq!(seen.author.username, "ferret42");
    assert_eq!(seen.author.role, Role::Guest);
    assert_eq!(ferret.message("hello from a guest").await.id, seen.id);

    caster.reply("welcome", Some(&seen.id));
    let reply = ferret.message("welcome").await;
    assert_eq!(reply.author.display_name, "Caster");
    assert_eq!(reply.author.role, Role::Broadcaster);
    assert_eq!(reply.author.badges, ["broadcaster"]);
    assert_eq!(reply.reply.unwrap().message_id, seen.id);

    elsewhere.say("different room");
    let first = elsewhere
        .until(|event| match event {
            ChatEvent::Message(message) => Some(message.content),
            _ => None,
        })
        .await;
    assert_eq!(first, "different room");
}

#[tokio::test]
async fn viewer_counts_follow_joins() {
    let server = Server::start().await;
    let mut caster = server.broadcaster("main").await;
    let _guest = server.guest("main", "ferret42").await;
    caster
        .until(|event| match event {
            ChatEvent::RoomState(state) if state.viewer_count == Some(2) => Some(()),
            _ => None,
        })
        .await;
}

#[tokio::test]
async fn a_moderator_can_delete_time_out_unban_and_clear() {
    let server = Server::start().await;
    let mut caster = server.broadcaster("main").await;
    let mut troll = server.guest("main", "troll").await;

    troll.say("bad message");
    let bad = caster.message("bad message").await;

    caster.command(ProviderCommand::Moderate(ModAction::Delete {
        message_id: bad.id.clone(),
    }));
    for client in [&mut caster, &mut troll] {
        let deleted = client
            .until(|event| match event {
                ChatEvent::MessageDeleted { message_id } => Some(message_id),
                _ => None,
            })
            .await;
        assert_eq!(deleted, bad.id);
    }

    let target = UserRef::of(&bad.author);
    caster.command(ProviderCommand::Moderate(ModAction::Timeout {
        target: target.clone(),
        secs: 60,
        reason: Some("spam".into()),
    }));
    let notice = moderated(&mut troll).await;
    assert_eq!(notice.action, Moderation::Timeout { secs: 60 });
    assert_eq!(notice.reason.as_deref(), Some("spam"));
    assert!(notice.target.matches(&bad.author));

    troll.say("let me in");
    let refusal = troll.system().await;
    assert!(
        refusal.starts_with("Not sent. You are timed out"),
        "{refusal}"
    );

    caster.command(ProviderCommand::Moderate(ModAction::Unban { target }));
    assert_eq!(moderated(&mut troll).await.action, Moderation::Unban);
    troll.say("sorry");
    caster.message("sorry").await;

    caster.command(ProviderCommand::Moderate(ModAction::Clear));
    troll
        .until(|event| (event == ChatEvent::ChatCleared).then_some(()))
        .await;
}

#[tokio::test]
async fn a_guest_cannot_moderate() {
    let server = Server::start().await;
    let mut guest = server.guest("main", "ferret42").await;
    guest.command(ProviderCommand::Moderate(ModAction::Clear));
    assert_eq!(
        guest.system().await,
        "Moderator or broadcaster role required."
    );
}

#[tokio::test]
async fn a_promoted_user_rejoins_as_moderator() {
    let server = Server::start().await;
    let mut caster = server.broadcaster("main").await;
    let jwt = || NativeAuth::Jwt(CLERK_JWT.into());
    let fan = server.joined("main", jwt(), Role::Member).await;
    fan.say("pick me");
    let author = caster.message("pick me").await.author;
    assert_eq!(author.username, "clerk_fan");
    assert_eq!(author.display_name, "Clerk Fan");

    caster.command(ProviderCommand::Moderate(ModAction::SetRole {
        target: UserRef::of(&author),
        role: Role::Moderator,
    }));
    caster.system().await;
    server.joined("main", jwt(), Role::Moderator).await;
}

#[tokio::test]
async fn a_rejected_token_stops_without_retrying() {
    let server = Server::start().await;
    let mut client = server.client(
        "main",
        NativeAuth::Token {
            token: "wrong".into(),
            display_name: None,
        },
    );
    assert_eq!(
        client.next().await,
        ChatEvent::ConnectionState(ConnectionState::Connecting)
    );
    assert_eq!(client.error().await, "Sign-in failed: invalid api token");
}

#[tokio::test]
async fn a_lurker_reads_history_but_cannot_send() {
    let server = Server::start().await;
    let mut caster = server.broadcaster("main").await;
    caster.say("before you came");
    caster.message("before you came").await;

    let mut lurker = server.client("main", NativeAuth::Lurker);
    assert!(!lurker.can_send());
    lurker.connected().await;
    lurker.message("before you came").await;
    lurker.say("hi");
    assert!(lurker.system().await.starts_with("Not sent. Sign in"));
}

#[tokio::test]
async fn room_emotes_arrive_on_join_and_tokenize() {
    let server = Server::start().await;
    let response = support::config()
        .http
        .post(format!("http://{}/api/v1/rooms/main/emotes", server.addr))
        .bearer_auth(ADMIN_TOKEN)
        .json(&serde_json::json!({
            "code": "ferretWave", "url": "https://cdn.example/wave.webp", "animated": true
        }))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());

    let key = "native:main".parse().unwrap();
    let mut store = ChatStore::new(100, &[]);
    let mut caster = server.client("main", admin());
    caster.connected().await;
    caster.say("hi ferretWave");
    loop {
        let event = caster.next().await;
        let done = matches!(event, ChatEvent::Message(_));
        store.apply(&key, event, OffsetDateTime::now_utc());
        if done {
            break;
        }
    }
    let rows = store.rows(&PaneMode::Single(key), &ViewState::default());
    let fragments = &rows.last().unwrap().message().unwrap().fragments;
    assert!(
        matches!(&fragments[1], MessageFragment::Emote(emote) if emote.animated),
        "{fragments:?}"
    );
}

#[tokio::test]
async fn a_server_restart_reconnects_as_the_same_guest_without_duplicating_history() {
    let mut server = Server::start().await;
    let key = "native:main".parse().unwrap();
    let mut store = ChatStore::new(100, &[]);
    let mut ferret = server.client("main", guest("ferret42"));
    ferret.connected().await;
    ferret.say("before");
    let before = ferret.message("before").await;
    store.apply(
        &key,
        ChatEvent::Message(before.clone()),
        OffsetDateTime::now_utc(),
    );

    server.restart().await;
    ferret.connected().await;
    ferret.say("after");
    let after = loop {
        let event = ferret.next().await;
        store.apply(&key, event.clone(), OffsetDateTime::now_utc());
        match event {
            ChatEvent::Message(message) if message.content == "after" => break message,
            _ => {}
        }
    };
    assert_eq!(after.author.id, before.author.id);

    let contents: Vec<String> = store
        .rows(&PaneMode::Single(key), &ViewState::default())
        .iter()
        .filter_map(|entry| Some(entry.message()?.message.content.clone()))
        .collect();
    assert_eq!(contents, ["before", "after"]);
}

#[tokio::test]
async fn deletion_from_the_server_strikes_the_message_in_the_store() {
    let server = Server::start().await;
    let key = "native:main".parse().unwrap();
    let mut store = ChatStore::new(100, &[]);
    let mut caster = server.broadcaster("main").await;
    caster.say("oops");
    let oops = caster.message("oops").await;
    caster.command(ProviderCommand::Moderate(ModAction::Delete {
        message_id: oops.id.clone(),
    }));
    store.apply(&key, ChatEvent::Message(oops), OffsetDateTime::now_utc());
    loop {
        let event = caster.next().await;
        let done = matches!(event, ChatEvent::MessageDeleted { .. });
        store.apply(&key, event, OffsetDateTime::now_utc());
        if done {
            break;
        }
    }
    let rows = store.rows(&PaneMode::Single(key), &ViewState::default());
    assert_eq!(rows[0].message().unwrap().removal, Some(Removal::Deleted));
}
