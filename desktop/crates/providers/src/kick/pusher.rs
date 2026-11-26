//! Kick's Pusher events.

use chat_core::{
    Author, ChatEvent, ChatMessage, Color, Emote, EmoteProvider, InlineEmote, MessageKind,
    Moderation, ReplyRef, Role, RoomState, UserModeration, UserRef,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub enum PusherEvent {
    /// The socket is ready for a subscription.
    Established,
    Subscribed,
    Ping,
    Failed(String),
    Chat(ChatEvent),
}

#[derive(Deserialize)]
struct Envelope {
    event: String,
    #[serde(default)]
    data: Value,
}

/// `None` for events this client has no use for.
pub fn parse_event(text: &str, now: OffsetDateTime) -> Option<PusherEvent> {
    let envelope: Envelope = serde_json::from_str(text).ok()?;
    let chat = match envelope.event.as_str() {
        "pusher:connection_established" => return Some(PusherEvent::Established),
        "pusher_internal:subscription_succeeded" => return Some(PusherEvent::Subscribed),
        "pusher:ping" => return Some(PusherEvent::Ping),
        "pusher:error" => {
            let error: PusherError = payload(envelope.data)?;
            return Some(PusherEvent::Failed(error.message));
        }
        "App\\Events\\ChatMessageEvent" => {
            let message: KickMessage = payload(envelope.data)?;
            ChatEvent::Message(message.into_domain(now))
        }
        "App\\Events\\MessageDeletedEvent" => {
            let deleted: MessageDeleted = payload(envelope.data)?;
            ChatEvent::MessageDeleted {
                message_id: deleted.message.id,
            }
        }
        "App\\Events\\UserBannedEvent" => {
            let banned: UserBanned = payload(envelope.data)?;
            banned.into_domain(now)?
        }
        "App\\Events\\UserUnbannedEvent" => {
            let unbanned: UserUnbanned = payload(envelope.data)?;
            ChatEvent::UserModerated(UserModeration {
                target: unbanned.user.target()?,
                action: Moderation::Unban,
                reason: None,
            })
        }
        "App\\Events\\ChatroomClearEvent" => ChatEvent::ChatCleared,
        "App\\Events\\ChatroomUpdatedEvent" => {
            let updated: ChatroomUpdated = payload(envelope.data)?;
            ChatEvent::RoomState(RoomState {
                slow_mode_secs: Some(updated.slow_mode.seconds()),
                ..RoomState::default()
            })
        }
        _ => return None,
    };
    Some(PusherEvent::Chat(chat))
}

/// Kick sends event data as JSON inside a JSON string.
fn payload<T: DeserializeOwned>(data: Value) -> Option<T> {
    match data {
        Value::String(text) => serde_json::from_str(&text).ok(),
        other => serde_json::from_value(other).ok(),
    }
}

#[derive(Deserialize)]
struct PusherError {
    message: String,
}

#[derive(Deserialize)]
struct KickMessage {
    id: String,
    content: String,
    created_at: String,
    sender: Sender,
    metadata: Option<Metadata>,
}

#[derive(Deserialize)]
struct Sender {
    id: u64,
    username: String,
    slug: String,
    identity: SenderIdentity,
}

#[derive(Deserialize)]
struct SenderIdentity {
    #[serde(default)]
    color: String,
    #[serde(default)]
    badges: Vec<KickBadge>,
}

#[derive(Deserialize)]
struct KickBadge {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct Metadata {
    original_sender: Option<OriginalSender>,
    original_message: Option<OriginalMessage>,
}

#[derive(Deserialize)]
struct OriginalSender {
    username: String,
}

#[derive(Deserialize)]
struct OriginalMessage {
    id: String,
    content: String,
}

impl KickMessage {
    fn into_domain(self, now: OffsetDateTime) -> ChatMessage {
        let (content, inline_emotes) = extract_emotes(&self.content);
        let reply = self.metadata.and_then(|metadata| {
            let original = metadata.original_message?;
            Some(ReplyRef {
                message_id: original.id,
                author: metadata.original_sender.map(|sender| sender.username),
                excerpt: Some(extract_emotes(&original.content).0),
            })
        });
        ChatMessage {
            id: self.id,
            author: self.sender.into_author(),
            content,
            inline_emotes,
            reply,
            kind: MessageKind::Chat,
            timestamp: parse_time(&self.created_at).unwrap_or(now),
        }
    }
}

impl Sender {
    fn into_author(self) -> Author {
        let badges: Vec<String> = self
            .identity
            .badges
            .into_iter()
            .map(|badge| badge.kind)
            .collect();
        let has = |name: &str| badges.iter().any(|badge| badge == name);
        let role = if has("broadcaster") {
            Role::Broadcaster
        } else if has("moderator") {
            Role::Moderator
        } else if has("vip") {
            Role::Vip
        } else {
            Role::Member
        };
        Author {
            id: self.id.to_string(),
            color: Color::parse(&self.identity.color)
                .unwrap_or_else(|| Color::for_name(&self.slug)),
            username: self.slug,
            display_name: self.username,
            badges,
            role,
        }
    }
}

#[derive(Deserialize)]
struct MessageDeleted {
    message: MessageId,
}

#[derive(Deserialize)]
struct MessageId {
    id: String,
}

#[derive(Deserialize)]
struct KickUser {
    id: u64,
    slug: String,
}

impl KickUser {
    fn target(self) -> Option<UserRef> {
        UserRef::new(Some(self.id.to_string()), Some(self.slug))
    }
}

#[derive(Deserialize)]
struct UserBanned {
    user: KickUser,
    #[serde(default)]
    permanent: bool,
    /// Minutes.
    duration: Option<u64>,
    expires_at: Option<String>,
}

impl UserBanned {
    fn into_domain(self, now: OffsetDateTime) -> Option<ChatEvent> {
        let expiry = self.expires_at.as_deref().and_then(parse_time);
        let secs = self
            .duration
            .map(|minutes| minutes * 60)
            .or(expiry.map(|at| (at - now).whole_seconds().max(0).unsigned_abs()));
        Some(ChatEvent::UserModerated(UserModeration {
            target: self.user.target()?,
            action: match secs {
                Some(secs) if !self.permanent => Moderation::Timeout { secs },
                _ => Moderation::Ban,
            },
            reason: None,
        }))
    }
}

#[derive(Deserialize)]
struct UserUnbanned {
    user: KickUser,
}

#[derive(Deserialize)]
struct ChatroomUpdated {
    slow_mode: SlowMode,
}

#[derive(Deserialize)]
pub struct SlowMode {
    enabled: bool,
    #[serde(default)]
    message_interval: u32,
}

impl SlowMode {
    fn seconds(&self) -> u32 {
        if self.enabled {
            self.message_interval
        } else {
            0
        }
    }
}

fn parse_time(text: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).ok()
}

/// Replaces each `[emote:id:name]` with its name and records where it went.
pub fn extract_emotes(raw: &str) -> (String, Vec<InlineEmote>) {
    let mut content = String::with_capacity(raw.len());
    let mut emotes = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("[emote:") {
        content.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some((id, name, length)) = emote_marker(rest) else {
            let (bracket, after) = rest.split_at(1);
            content.push_str(bracket);
            rest = after;
            continue;
        };
        let code = if name.is_empty() { "emote" } else { name };
        let begin = content.len();
        content.push_str(code);
        emotes.push(InlineEmote {
            range: begin..content.len(),
            emote: Emote {
                code: code.to_owned(),
                url: format!("https://files.kick.com/emotes/{id}/fullsize"),
                animated: false,
                provider: EmoteProvider::Kick,
            },
        });
        rest = &rest[length..];
    }
    content.push_str(rest);
    (content, emotes)
}

/// Reads a marker at the start of `text`: its id, name, and full length.
fn emote_marker(text: &str) -> Option<(&str, &str, usize)> {
    let end = text.find(']')?;
    let (id, name) = text["[emote:".len()..end].split_once(':')?;
    let numeric = !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit());
    numeric.then_some((id, name, end + 1))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use time::macros::datetime;

    use super::*;

    const NOW: OffsetDateTime = datetime!(2026-11-25 12:00 UTC);

    /// Wraps `data` the way Pusher does: JSON text inside JSON.
    fn event(name: &str, data: Value) -> Option<PusherEvent> {
        let frame = json!({
            "event": name,
            "data": data.to_string(),
            "channel": "chatrooms.1.v2",
        });
        parse_event(&frame.to_string(), NOW)
    }

    fn chat(name: &str, data: Value) -> ChatEvent {
        match event(name, data) {
            Some(PusherEvent::Chat(chat)) => chat,
            other => panic!("not a chat event: {other:?}"),
        }
    }

    fn chat_message(extra: Value) -> ChatMessage {
        let mut data = json!({
            "id": "uuid-1",
            "chatroom_id": 1,
            "content": "nice [emote:37226:KEKW] one",
            "type": "message",
            "created_at": "2026-11-25T11:59:58+00:00",
            "sender": {
                "id": 42, "username": "Some_One", "slug": "some-one",
                "identity": { "color": "#75FD46", "badges": [
                    { "type": "moderator", "text": "Moderator" },
                    { "type": "subscriber", "text": "Subscriber", "count": 3 }
                ]}
            }
        });
        data.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        match chat("App\\Events\\ChatMessageEvent", data) {
            ChatEvent::Message(message) => message,
            other => panic!("not a message: {other:?}"),
        }
    }

    #[test]
    fn a_chat_message_carries_author_badges_and_emotes() {
        let message = chat_message(json!({}));
        assert_eq!(message.content, "nice KEKW one");
        assert_eq!(message.author.id, "42");
        assert_eq!(message.author.username, "some-one");
        assert_eq!(message.author.display_name, "Some_One");
        assert_eq!(message.author.color, Color(0x75fd46));
        assert_eq!(message.author.badges, ["moderator", "subscriber"]);
        assert_eq!(message.author.role, Role::Moderator);
        assert_eq!(message.timestamp, datetime!(2026-11-25 11:59:58 UTC));

        let placed = &message.inline_emotes[0];
        assert_eq!(&message.content[placed.range.clone()], "KEKW");
        assert_eq!(
            placed.emote.url,
            "https://files.kick.com/emotes/37226/fullsize"
        );
    }

    #[test]
    fn a_reply_names_its_parent() {
        let message = chat_message(json!({
            "type": "reply",
            "metadata": {
                "original_sender": { "id": 7, "username": "Other" },
                "original_message": { "id": "uuid-0", "content": "[emote:1:Hi] there" }
            }
        }));
        assert_eq!(
            message.reply,
            Some(ReplyRef {
                message_id: "uuid-0".into(),
                author: Some("Other".into()),
                excerpt: Some("Hi there".into()),
            })
        );
    }

    #[test]
    fn connection_events_are_recognized() {
        assert_eq!(
            event("pusher:connection_established", json!({"socket_id": "1.2"})),
            Some(PusherEvent::Established)
        );
        assert_eq!(
            parse_event(
                r#"{"event":"pusher_internal:subscription_succeeded","channel":"chatrooms.1.v2","data":"{}"}"#,
                NOW
            ),
            Some(PusherEvent::Subscribed)
        );
        assert_eq!(
            parse_event(r#"{"event":"pusher:ping","data":{}}"#, NOW),
            Some(PusherEvent::Ping)
        );
        assert_eq!(
            parse_event(
                r#"{"event":"pusher:error","data":{"code":4001,"message":"no app"}}"#,
                NOW
            ),
            Some(PusherEvent::Failed("no app".into()))
        );
    }

    #[test]
    fn moderation_events_map_to_the_domain() {
        assert_eq!(
            chat(
                "App\\Events\\MessageDeletedEvent",
                json!({"id": "event-id", "message": {"id": "uuid-1"}})
            ),
            ChatEvent::MessageDeleted {
                message_id: "uuid-1".into()
            }
        );
        assert_eq!(
            chat("App\\Events\\ChatroomClearEvent", json!({"id": "x"})),
            ChatEvent::ChatCleared
        );

        let user = json!({"id": 7, "username": "Troll", "slug": "troll"});
        let moderated = |action| {
            ChatEvent::UserModerated(UserModeration {
                target: UserRef::new(Some("7".into()), Some("troll".into())).unwrap(),
                action,
                reason: None,
            })
        };
        let banned = "App\\Events\\UserBannedEvent";
        assert_eq!(
            chat(banned, json!({"user": user, "permanent": true})),
            moderated(Moderation::Ban)
        );
        assert_eq!(
            chat(
                banned,
                json!({"user": user, "permanent": false, "duration": 5})
            ),
            moderated(Moderation::Timeout { secs: 300 })
        );
        assert_eq!(
            chat(
                banned,
                json!({"user": user, "expires_at": "2026-11-25T12:01:00Z"})
            ),
            moderated(Moderation::Timeout { secs: 60 })
        );
        assert_eq!(
            chat("App\\Events\\UserUnbannedEvent", json!({"user": user})),
            moderated(Moderation::Unban)
        );
    }

    #[test]
    fn slow_mode_off_reads_as_zero() {
        for (enabled, expected) in [(true, 10), (false, 0)] {
            let event = chat(
                "App\\Events\\ChatroomUpdatedEvent",
                json!({"slow_mode": {"enabled": enabled, "message_interval": 10}}),
            );
            let ChatEvent::RoomState(state) = event else {
                panic!("expected room state");
            };
            assert_eq!(state.slow_mode_secs, Some(expected));
        }
    }

    #[test]
    fn unknown_and_malformed_events_are_skipped() {
        assert_eq!(event("App\\Events\\SomethingNew", json!({})), None);
        assert_eq!(
            event("App\\Events\\ChatMessageEvent", json!({"id": 1})),
            None
        );
        assert_eq!(parse_event("not json", NOW), None);
    }

    #[test]
    fn emote_markers_that_do_not_parse_stay_text() {
        for raw in [
            "[emote:",
            "[emote:abc:x]",
            "[emote:1]",
            "[emote::x]",
            "é [emote:1:x",
        ] {
            assert_eq!(extract_emotes(raw), (raw.to_owned(), vec![]), "{raw}");
        }
        let (content, emotes) = extract_emotes("[emote:1:a][emote:2:]é");
        assert_eq!(content, "aemoteé");
        assert_eq!(emotes[1].range, 1..6);
    }
}
