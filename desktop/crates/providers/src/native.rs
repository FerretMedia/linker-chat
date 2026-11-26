use std::time::Duration;

use chat_core::{
    Author, ChannelKey, ChatEvent, ChatMessage, Color, Emote, EmoteProvider, Identity, MessageKind,
    ModAction, Moderation, ReplyRef, Role, RoomState, UserModeration, UserRef,
};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::config::{NativeAuth, NativeConfig, ProviderConfig};
use crate::provider::{ChatProvider, ProviderCommand, SessionError, SessionIo};
use crate::socket::Socket;

const PING_EVERY: Duration = Duration::from_secs(20);
const IDLE_LIMIT: Duration = Duration::from_secs(50);
const AUTH_REQUEST: &str = "auth";
const SEND_REQUEST: &str = "send";

/// Linker-chat wire protocol v1 over one websocket per room.
pub struct NativeProvider {
    channel: ChannelKey,
    config: NativeConfig,
    /// Keeps the guest identity across reconnects.
    guest_token: Option<String>,
    user: Option<wire::User>,
}

impl NativeProvider {
    pub fn new(channel: ChannelKey, config: &ProviderConfig) -> Self {
        Self {
            channel,
            config: config.native.clone(),
            guest_token: None,
            user: None,
        }
    }

    fn auth_frame(&self) -> Option<String> {
        let (kind, data) = match &self.config.auth {
            NativeAuth::Lurker => return None,
            NativeAuth::Guest { nickname } => (
                "auth_guest",
                json!({ "nickname": nickname, "guest_token": self.guest_token }),
            ),
            NativeAuth::Token {
                token,
                display_name,
            } => (
                "auth_token",
                json!({ "token": token, "display_name": display_name }),
            ),
            NativeAuth::Jwt(token) => ("auth", json!({ "token": token })),
        };
        Some(frame(kind, AUTH_REQUEST, data))
    }

    async fn session(&mut self, io: &mut SessionIo) -> Result<(), SessionError> {
        let mut socket = Socket::connect(&self.config.server_url, IDLE_LIMIT).await?;
        if let Some(auth) = self.auth_frame() {
            socket.send(auth).await?;
        }
        // The server handles frames in order, so `join` sees the signed-in role.
        let room = json!({ "room": self.channel.channel() });
        socket.send(frame("join", "join", room)).await?;

        let mut ping = tokio::time::interval(PING_EVERY);
        loop {
            tokio::select! {
                text = socket.next() => self.receive(&text?, io)?,
                _ = ping.tick() => socket.send(frame("ping", "", json!({}))).await?,
                command = io.command() => match command {
                    Some(command) => socket.send(encode(self.channel.channel(), command)).await?,
                    None => return Ok(()),
                },
            }
        }
    }

    fn receive(&mut self, text: &str, io: &mut SessionIo) -> Result<(), SessionError> {
        let Some((frame, req_id)) = decode(text) else {
            return Ok(());
        };
        match frame {
            Frame::AuthOk(auth) => {
                self.guest_token = auth.guest_token.or(self.guest_token.take());
                self.user = Some(auth.user);
            }
            Frame::Error(error) if req_id == AUTH_REQUEST => {
                return Err(SessionError::Fatal(format!(
                    "Sign-in failed: {}",
                    error.message
                )));
            }
            Frame::Error(error) if req_id == SEND_REQUEST => {
                io.system(format!("Not sent. {}", error.message));
            }
            Frame::Error(error) => io.system(error.message),
            Frame::RoomJoined(joined) => {
                io.connected();
                if let Some(user) = &self.user {
                    io.emit(ChatEvent::Identity(Identity {
                        username: user.username.clone(),
                        display_name: user.display_name.clone(),
                        role: parse_role(&joined.role),
                    }));
                }
                io.emit_all(joined.into_events());
            }
            Frame::Event(event) => {
                if event.room.is_none_or(|room| room == self.channel.channel()) {
                    io.emit(event.event);
                }
            }
        }
        Ok(())
    }
}

impl ChatProvider for NativeProvider {
    fn channel(&self) -> &ChannelKey {
        &self.channel
    }

    fn can_send(&self) -> bool {
        !matches!(self.config.auth, NativeAuth::Lurker)
    }

    fn run_session<'a>(
        &'a mut self,
        io: &'a mut SessionIo,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(self.session(io))
    }
}

fn frame(kind: &str, req_id: &str, data: Value) -> String {
    json!({ "type": kind, "req_id": req_id, "data": data }).to_string()
}

fn encode(room: &str, command: ProviderCommand) -> String {
    match command {
        ProviderCommand::Send { content, reply_to } => frame(
            "send_message",
            SEND_REQUEST,
            json!({ "room": room, "content": content, "reply_to_id": reply_to }),
        ),
        ProviderCommand::Moderate(action) => {
            frame("moderate", "moderate", moderate_request(room, &action))
        }
    }
}

fn moderate_request(room: &str, action: &ModAction) -> Value {
    let on_user = |action: &str, target: &UserRef| {
        json!({
            "room": room,
            "action": action,
            "target_user_id": target.id(),
            "target_username": target.username(),
        })
    };
    let mut request = match action {
        ModAction::Delete { message_id } => {
            json!({ "room": room, "action": "delete", "message_id": message_id })
        }
        ModAction::Clear => json!({ "room": room, "action": "clear" }),
        ModAction::Timeout { target, secs, .. } => {
            let mut request = on_user("timeout", target);
            request["duration_sec"] = json!(secs);
            request
        }
        ModAction::Ban { target, .. } => on_user("ban", target),
        ModAction::Unban { target } => on_user("unban", target),
        ModAction::SetRole { target, role } => {
            let mut request = on_user("set_role", target);
            request["role"] = json!(role_name(*role));
            request
        }
    };
    if let ModAction::Timeout {
        reason: Some(reason),
        ..
    }
    | ModAction::Ban {
        reason: Some(reason),
        ..
    } = action
    {
        request["reason"] = json!(reason);
    }
    request
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Broadcaster => "broadcaster",
        Role::Moderator => "moderator",
        Role::Vip => "vip",
        Role::Member | Role::Guest => "member",
    }
}

fn parse_role(name: &str) -> Role {
    match name {
        "broadcaster" | "admin" => Role::Broadcaster,
        "moderator" => Role::Moderator,
        "vip" => Role::Vip,
        "guest" => Role::Guest,
        _ => Role::Member,
    }
}

/// A server frame, with everything the domain already has a word for as an event.
enum Frame {
    AuthOk(wire::AuthOk),
    RoomJoined(wire::RoomJoined),
    Error(wire::ErrorPayload),
    Event(RoomEvent),
}

struct RoomEvent {
    room: Option<String>,
    event: ChatEvent,
}

/// `None` for pongs and for frames this client does not understand.
fn decode(text: &str) -> Option<(Frame, String)> {
    let envelope: wire::Envelope = serde_json::from_str(text).ok()?;
    fn data<T: for<'de> Deserialize<'de>>(value: Value) -> Option<T> {
        serde_json::from_value(value).ok()
    }
    let in_room = |room: String, event| {
        Frame::Event(RoomEvent {
            room: Some(room),
            event,
        })
    };
    let frame = match envelope.kind.as_str() {
        "auth_ok" => Frame::AuthOk(data(envelope.data)?),
        "room_joined" => Frame::RoomJoined(data(envelope.data)?),
        "error" => Frame::Error(data(envelope.data)?),
        "message" => {
            let message: wire::Message = data(envelope.data)?;
            in_room(message.room.clone(), ChatEvent::Message(message.into()))
        }
        "message_deleted" => {
            let deleted: wire::MessageDeleted = data(envelope.data)?;
            in_room(
                deleted.room,
                ChatEvent::MessageDeleted {
                    message_id: deleted.message_id,
                },
            )
        }
        "user_moderated" => {
            let moderated: wire::UserModerated = data(envelope.data)?;
            let room = moderated.room.clone();
            in_room(room, ChatEvent::UserModerated(moderated.into_domain()?))
        }
        "chat_cleared" => {
            let cleared: wire::ChatCleared = data(envelope.data)?;
            in_room(cleared.room, ChatEvent::ChatCleared)
        }
        "room_state" => {
            let state: wire::RoomState = data(envelope.data)?;
            let room = state.room.clone();
            in_room(room, ChatEvent::RoomState(state.into()))
        }
        "system" => {
            let system: wire::System = data(envelope.data)?;
            Frame::Event(RoomEvent {
                room: system.room,
                event: ChatEvent::System(system.text),
            })
        }
        _ => return None,
    };
    Some((frame, envelope.req_id))
}

mod wire {
    use super::*;

    #[derive(Deserialize)]
    pub struct Envelope {
        #[serde(rename = "type")]
        pub kind: String,
        #[serde(default)]
        pub req_id: String,
        #[serde(default)]
        pub data: Value,
    }

    #[derive(Deserialize, Clone)]
    pub struct User {
        pub id: String,
        pub username: String,
        pub display_name: String,
        #[serde(default)]
        pub color: String,
        #[serde(default)]
        pub role: String,
        #[serde(default)]
        pub is_guest: bool,
    }

    #[derive(Deserialize)]
    pub struct AuthOk {
        pub user: User,
        pub guest_token: Option<String>,
    }

    #[derive(Deserialize)]
    pub struct ErrorPayload {
        pub message: String,
    }

    #[derive(Deserialize)]
    pub struct WireEmote {
        code: String,
        url: String,
        #[serde(default)]
        animated: bool,
    }

    #[derive(Deserialize)]
    pub struct RoomJoined {
        state: RoomState,
        pub role: String,
        emotes: Option<Vec<WireEmote>>,
        recent_messages: Option<Vec<Message>>,
    }

    impl RoomJoined {
        pub fn into_events(self) -> impl Iterator<Item = ChatEvent> {
            let emotes = self.emotes.unwrap_or_default().into_iter().map(Emote::from);
            [
                ChatEvent::RoomState(self.state.into()),
                ChatEvent::Emotes(emotes.collect()),
            ]
            .into_iter()
            .chain(
                self.recent_messages
                    .unwrap_or_default()
                    .into_iter()
                    .map(|message| ChatEvent::Message(message.into())),
            )
        }
    }

    impl From<WireEmote> for Emote {
        fn from(emote: WireEmote) -> Self {
            Emote {
                code: emote.code,
                url: emote.url,
                animated: emote.animated,
                provider: EmoteProvider::Native,
            }
        }
    }

    #[derive(Deserialize)]
    pub struct RoomState {
        pub room: String,
        title: String,
        slow_mode_sec: u32,
        allow_guests: bool,
        viewer_count: u64,
    }

    impl From<RoomState> for super::RoomState {
        fn from(state: RoomState) -> Self {
            Self {
                title: Some(state.title),
                slow_mode_secs: Some(state.slow_mode_sec),
                guests_allowed: Some(state.allow_guests),
                viewer_count: Some(state.viewer_count),
            }
        }
    }

    #[derive(Deserialize)]
    pub struct Message {
        id: String,
        pub room: String,
        user: User,
        content: String,
        #[serde(default)]
        reply_to_id: String,
        badges: Option<Vec<String>>,
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
    }

    impl From<Message> for ChatMessage {
        fn from(message: Message) -> Self {
            let user = message.user;
            let role = if user.is_guest {
                parse_role(if user.role.is_empty() {
                    "guest"
                } else {
                    &user.role
                })
            } else {
                parse_role(&user.role)
            };
            ChatMessage {
                id: message.id,
                author: Author {
                    color: Color::parse(&user.color)
                        .unwrap_or_else(|| Color::for_name(&user.username)),
                    id: user.id,
                    username: user.username,
                    display_name: user.display_name,
                    badges: message.badges.unwrap_or_default(),
                    role,
                },
                content: message.content,
                inline_emotes: Vec::new(),
                reply: Some(message.reply_to_id)
                    .filter(|id| !id.is_empty())
                    .map(|message_id| ReplyRef {
                        message_id,
                        author: None,
                        excerpt: None,
                    }),
                kind: MessageKind::Chat,
                timestamp: message.created_at,
            }
        }
    }

    #[derive(Deserialize)]
    pub struct MessageDeleted {
        pub room: String,
        pub message_id: String,
    }

    #[derive(Deserialize)]
    pub struct UserModerated {
        pub room: String,
        action: String,
        target_user_id: Option<String>,
        target_username: Option<String>,
        duration_sec: Option<u64>,
        reason: Option<String>,
    }

    impl UserModerated {
        pub fn into_domain(self) -> Option<UserModeration> {
            let action = match self.action.as_str() {
                "timeout" => Moderation::Timeout {
                    secs: self.duration_sec.unwrap_or_default(),
                },
                "ban" => Moderation::Ban,
                "unban" => Moderation::Unban,
                _ => return None,
            };
            Some(UserModeration {
                target: UserRef::new(self.target_user_id, self.target_username)?,
                action,
                reason: self.reason,
            })
        }
    }

    #[derive(Deserialize)]
    pub struct ChatCleared {
        pub room: String,
    }

    #[derive(Deserialize)]
    pub struct System {
        pub room: Option<String>,
        pub text: String,
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn event(text: &str) -> ChatEvent {
        match decode(text) {
            Some((Frame::Event(event), _)) => event.event,
            _ => panic!("not an event: {text}"),
        }
    }

    #[test]
    fn decodes_a_message_frame() {
        let text = r##"{"type":"message","data":{"id":"msg_1","room":"main",
            "user":{"id":"admin:broadcaster","username":"caster","display_name":"Caster",
                    "color":"#A78BFA","role":"broadcaster","is_guest":false,"is_admin":true},
            "content":"hello","reply_to_id":"msg_0","badges":["broadcaster"],
            "created_at":"2026-11-25T12:00:00.123456789Z"}}"##;
        let ChatEvent::Message(message) = event(text) else {
            panic!("expected a message");
        };
        assert_eq!(message.id, "msg_1");
        assert_eq!(message.author.role, Role::Broadcaster);
        assert_eq!(message.author.color, Color(0xa78bfa));
        assert_eq!(message.reply.unwrap().message_id, "msg_0");
        assert_eq!(
            message.timestamp,
            datetime!(2026-11-25 12:00:00.123456789 UTC)
        );
    }

    #[test]
    fn decodes_moderation_frames() {
        assert_eq!(
            event(
                r#"{"type":"user_moderated","data":{"room":"main","action":"timeout",
                    "target_username":"troll","duration_sec":60,"reason":"spam","actor_id":"a"}}"#
            ),
            ChatEvent::UserModerated(UserModeration {
                target: UserRef::new(None, Some("troll".into())).unwrap(),
                action: Moderation::Timeout { secs: 60 },
                reason: Some("spam".into()),
            })
        );
        assert_eq!(
            event(
                r#"{"type":"message_deleted","data":{"room":"main","message_id":"m","actor_id":"a"}}"#
            ),
            ChatEvent::MessageDeleted {
                message_id: "m".into()
            }
        );
        assert_eq!(
            event(r#"{"type":"chat_cleared","data":{"room":"main","actor_id":"a"}}"#),
            ChatEvent::ChatCleared
        );
    }

    #[test]
    fn a_join_snapshot_tolerates_null_lists() {
        let text = r#"{"type":"room_joined","req_id":"join","data":{"room":"main",
            "state":{"room":"main","title":"Main","slow_mode_sec":5,"allow_guests":true,
                     "viewer_count":3},
            "role":"guest","viewer_count":3,"emotes":null,"recent_messages":null}}"#;
        let Some((Frame::RoomJoined(joined), _)) = decode(text) else {
            panic!("expected a join snapshot");
        };
        let events: Vec<_> = joined.into_events().collect();
        let ChatEvent::RoomState(state) = &events[0] else {
            panic!("expected room state first");
        };
        assert_eq!(state.slow_mode_secs, Some(5));
        assert_eq!(state.viewer_count, Some(3));
    }

    #[test]
    fn unknown_and_malformed_frames_are_skipped() {
        for text in [
            "",
            "not json",
            r#"{"type":"pong","data":{}}"#,
            r#"{"type":"from_the_future","data":{}}"#,
            r#"{"type":"message","data":{"id":"missing the rest"}}"#,
            r#"{"type":"user_moderated","data":{"room":"main","action":"ban","actor_id":"a"}}"#,
        ] {
            assert!(decode(text).is_none(), "{text}");
        }
    }

    #[test]
    fn moderation_requests_carry_the_protocol_fields() {
        let target = UserRef::new(Some("guest:1".into()), Some("troll".into())).unwrap();
        assert_eq!(
            moderate_request(
                "main",
                &ModAction::Timeout {
                    target: target.clone(),
                    secs: 600,
                    reason: Some("spam".into())
                }
            ),
            json!({ "room": "main", "action": "timeout", "target_user_id": "guest:1",
                    "target_username": "troll", "duration_sec": 600, "reason": "spam" })
        );
        assert_eq!(
            moderate_request(
                "main",
                &ModAction::SetRole {
                    target,
                    role: Role::Vip
                }
            ),
            json!({ "room": "main", "action": "set_role", "target_user_id": "guest:1",
                    "target_username": "troll", "role": "vip" })
        );
    }
}
