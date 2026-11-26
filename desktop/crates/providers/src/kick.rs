mod pusher;

use std::time::Duration;

use chat_core::{ChannelKey, ChatEvent, RoomState};
use futures::future::BoxFuture;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
use time::OffsetDateTime;

use crate::config::ProviderConfig;
use crate::provider::{ChatProvider, ProviderCommand, SessionError, SessionIo};
use crate::socket::{Socket, dropped};
use crate::third_party;
use pusher::{PusherEvent, parse_event};

const PING_EVERY: Duration = Duration::from_secs(60);
const CLIENT_ID: &str = "linker-chat-desktop";
/// Pusher promises traffic within 120 seconds of a ping.
const IDLE_LIMIT: Duration = Duration::from_secs(180);

/// Kick chat over Pusher, with sending through the official API.
pub struct KickProvider {
    channel: ChannelKey,
    config: ProviderConfig,
    room: Option<Room>,
}

#[derive(Clone, Copy)]
struct Room {
    chatroom_id: u64,
    broadcaster_user_id: Option<u64>,
}

#[derive(Deserialize)]
struct ChannelInfo {
    user_id: u64,
    chatroom: Chatroom,
    livestream: Option<Livestream>,
}

#[derive(Deserialize)]
struct Chatroom {
    id: u64,
}

#[derive(Deserialize)]
struct Livestream {
    session_title: Option<String>,
    viewer_count: Option<u64>,
}

#[derive(Deserialize)]
struct RealtimeGrant {
    data: RealtimeConnections,
}

#[derive(Deserialize)]
struct RealtimeConnections {
    connections: Vec<RealtimeConnection>,
}

#[derive(Deserialize)]
#[serde(tag = "provider", rename_all = "lowercase")]
enum RealtimeConnection {
    Pusher {
        credentials: PusherApp,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct PusherApp {
    app_key: String,
    cluster: String,
}

impl KickProvider {
    pub fn new(channel: ChannelKey, config: &ProviderConfig) -> Self {
        Self {
            channel,
            config: config.clone(),
            room: None,
        }
    }

    async fn session(&mut self, io: &mut SessionIo) -> Result<(), SessionError> {
        let room = match self.room {
            Some(room) => room,
            None => self.find_room(io).await?,
        };
        self.room = Some(room);

        let url = self.pusher_url().await?;
        let mut socket = Socket::connect(&url, IDLE_LIMIT).await?;
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.tick().await;
        loop {
            tokio::select! {
                text = socket.next() => match parse_event(&text?, OffsetDateTime::now_utc()) {
                    Some(PusherEvent::Established) => socket.send(subscribe(room)).await?,
                    Some(PusherEvent::Subscribed) => io.connected(),
                    Some(PusherEvent::Ping) => socket.send(control("pusher:pong")).await?,
                    Some(PusherEvent::Failed(reason)) => return Err(dropped(reason)),
                    Some(PusherEvent::Chat(event)) => io.emit(event),
                    None => {}
                },
                _ = ping.tick() => socket.send(control("pusher:ping")).await?,
                command = io.command() => match command {
                    None => return Ok(()),
                    Some(ProviderCommand::Moderate(_)) => {
                        io.system("Moderating Kick from here is not supported.");
                    }
                    Some(ProviderCommand::Send { content, reply_to }) => {
                        if let Err(reason) = self.send(room, &content, reply_to).await {
                            io.system(format!("Not sent. {reason}"));
                        }
                    }
                },
            }
        }
    }

    /// Asks which Pusher app to use. Kick rotates it, so this runs per connection.
    async fn pusher_url(&self) -> Result<String, SessionError> {
        let endpoints = &self.config.endpoints;
        let response = self
            .config
            .http
            .post(format!(
                "{}/api/v1/realtime/connection",
                endpoints.kick_realtime
            ))
            .json(&json!({
                "client": { "id": CLIENT_ID, "type": "web" },
                "capabilities": { "accepted_providers": [{ "provider": "pusher" }] },
            }))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(http_error)?;
        let grant: RealtimeGrant = response.json().await.map_err(http_error)?;
        let PusherApp { app_key, cluster } = grant
            .data
            .connections
            .into_iter()
            .find_map(|connection| match connection {
                RealtimeConnection::Pusher { credentials } => Some(credentials),
                RealtimeConnection::Other => None,
            })
            .ok_or_else(|| SessionError::Fatal("Kick no longer offers chat over Pusher.".into()))?;
        let host = endpoints.kick_pusher.replace("{cluster}", &cluster);
        Ok(format!(
            "{host}/app/{app_key}?protocol=7&client={CLIENT_ID}&version={}",
            env!("CARGO_PKG_VERSION")
        ))
    }

    /// Looks up the chatroom by slug. An all-digit channel is a chatroom id
    /// already, for when Kick refuses the lookup.
    async fn find_room(&self, io: &SessionIo) -> Result<Room, SessionError> {
        let slug = self.channel.channel();
        if let Ok(chatroom_id) = slug.parse() {
            return Ok(Room {
                chatroom_id,
                broadcaster_user_id: None,
            });
        }
        let url = format!("{}/api/v2/channels/{slug}", self.config.endpoints.kick_site);
        let response = self.config.http.get(url).send().await.map_err(http_error)?;
        match response.status() {
            StatusCode::OK => {}
            StatusCode::NOT_FOUND => {
                return Err(SessionError::Fatal(format!(
                    "There is no Kick channel named {slug}."
                )));
            }
            StatusCode::FORBIDDEN => {
                return Err(SessionError::Fatal(format!(
                    "Kick refused to look up {slug}. Connect with its numeric chatroom id instead."
                )));
            }
            status => return Err(dropped(format!("Kick answered {status}"))),
        }
        let info: ChannelInfo = response.json().await.map_err(http_error)?;
        if let Some(livestream) = info.livestream {
            io.emit(ChatEvent::RoomState(RoomState {
                title: livestream.session_title,
                viewer_count: livestream.viewer_count,
                ..RoomState::default()
            }));
        }
        if self.config.third_party_emotes {
            let http = self.config.http.clone();
            io.load_emotes(
                async move { third_party::fetch_kick_channel(&http, info.user_id).await },
            );
        }
        Ok(Room {
            chatroom_id: info.chatroom.id,
            broadcaster_user_id: Some(info.user_id),
        })
    }

    async fn send(
        &self,
        room: Room,
        content: &str,
        reply_to: Option<String>,
    ) -> Result<(), String> {
        let token = self
            .config
            .kick_token
            .as_ref()
            .ok_or("Sign in to Kick to chat here.")?;
        let broadcaster = room
            .broadcaster_user_id
            .ok_or("Sending needs the channel name, not its chatroom id.")?;
        let mut body = json!({
            "broadcaster_user_id": broadcaster,
            "content": content,
            "type": "user",
        });
        if let Some(id) = reply_to {
            body["reply_to_message_id"] = json!(id);
        }
        let url = format!("{}/public/v1/chat", self.config.endpoints.kick_api);
        let response = self
            .config
            .http
            .post(url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|error| error.without_url().to_string())?;
        match response.status() {
            status if status.is_success() => Ok(()),
            status => Err(format!("Kick answered {status}.")),
        }
    }
}

impl ChatProvider for KickProvider {
    fn channel(&self) -> &ChannelKey {
        &self.channel
    }

    fn can_send(&self) -> bool {
        self.config.kick_token.is_some()
    }

    fn run_session<'a>(
        &'a mut self,
        io: &'a mut SessionIo,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(self.session(io))
    }
}

fn http_error(error: reqwest::Error) -> SessionError {
    dropped(error.without_url())
}

fn subscribe(room: Room) -> String {
    let channel = format!("chatrooms.{}.v2", room.chatroom_id);
    json!({ "event": "pusher:subscribe", "data": { "auth": "", "channel": channel } }).to_string()
}

fn control(event: &str) -> String {
    json!({ "event": event, "data": {} }).to_string()
}
