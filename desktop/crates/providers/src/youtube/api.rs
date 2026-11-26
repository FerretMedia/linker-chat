//! YouTube Data API v3 `liveChatMessages` resources.

use chat_core::{
    Author, ChatEvent, ChatMessage, Color, MessageKind, Moderation, Role, UserModeration, UserRef,
};
use serde::Deserialize;
use time::OffsetDateTime;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoList {
    #[serde(default)]
    pub items: Vec<Video>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Video {
    pub snippet: VideoSnippet,
    pub live_streaming_details: Option<LiveStreamingDetails>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoSnippet {
    pub title: String,
    pub channel_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStreamingDetails {
    pub active_live_chat_id: Option<String>,
    /// The API sends counts as strings.
    concurrent_viewers: Option<String>,
}

impl LiveStreamingDetails {
    pub fn viewers(&self) -> Option<u64> {
        self.concurrent_viewers.as_deref()?.parse().ok()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub next_page_token: Option<String>,
    pub polling_interval_millis: Option<u64>,
    pub offline_at: Option<String>,
    #[serde(default)]
    pub items: Vec<LiveChatMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveChatMessage {
    id: String,
    snippet: Snippet,
    author_details: Option<AuthorDetails>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snippet {
    #[serde(rename = "type")]
    kind: String,
    #[serde(with = "time::serde::rfc3339")]
    published_at: OffsetDateTime,
    display_message: Option<String>,
    super_chat_details: Option<SuperChatDetails>,
    super_sticker_details: Option<SuperStickerDetails>,
    new_sponsor_details: Option<NewSponsorDetails>,
    member_milestone_chat_details: Option<MemberMilestoneDetails>,
    membership_gifting_details: Option<MembershipGiftingDetails>,
    message_deleted_details: Option<MessageDeletedDetails>,
    user_banned_details: Option<UserBannedDetails>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuperChatDetails {
    amount_display_string: String,
    #[serde(default)]
    user_comment: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuperStickerDetails {
    amount_display_string: String,
    super_sticker_metadata: StickerMetadata,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StickerMetadata {
    #[serde(default)]
    alt_text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewSponsorDetails {
    #[serde(default)]
    member_level_name: String,
    #[serde(default)]
    is_upgrade: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemberMilestoneDetails {
    member_month: u32,
    #[serde(default)]
    user_comment: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MembershipGiftingDetails {
    gift_memberships_count: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageDeletedDetails {
    deleted_message_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserBannedDetails {
    banned_user_details: BannedUser,
    ban_type: String,
    /// The API sends 64-bit numbers as strings.
    ban_duration_seconds: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BannedUser {
    channel_id: String,
    #[serde(default)]
    display_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthorDetails {
    channel_id: String,
    display_name: String,
    #[serde(default)]
    is_verified: bool,
    #[serde(default)]
    is_chat_owner: bool,
    #[serde(default)]
    is_chat_sponsor: bool,
    #[serde(default)]
    is_chat_moderator: bool,
}

impl From<AuthorDetails> for Author {
    fn from(details: AuthorDetails) -> Self {
        let role = if details.is_chat_owner {
            Role::Broadcaster
        } else if details.is_chat_moderator {
            Role::Moderator
        } else {
            Role::Member
        };
        let badges = [
            (details.is_chat_owner, "owner"),
            (details.is_chat_moderator, "moderator"),
            (details.is_chat_sponsor, "member"),
            (details.is_verified, "verified"),
        ];
        Author {
            id: details.channel_id,
            color: Color::for_name(&details.display_name),
            username: details.display_name.trim_start_matches('@').to_owned(),
            display_name: details.display_name,
            badges: badges
                .into_iter()
                .filter(|(held, _)| *held)
                .map(|(_, name)| name.to_owned())
                .collect(),
            role,
        }
    }
}

impl LiveChatMessage {
    /// `None` for housekeeping items and anything missing its own details.
    pub fn into_event(self) -> Option<ChatEvent> {
        let snippet = self.snippet;
        let display = snippet.display_message.unwrap_or_default();
        let (kind, content) = match snippet.kind.as_str() {
            "textMessageEvent" => (MessageKind::Chat, display),
            "superChatEvent" => {
                let details = snippet.super_chat_details?;
                let amount = details.amount_display_string;
                (MessageKind::Paid { amount }, details.user_comment)
            }
            "superStickerEvent" => {
                let details = snippet.super_sticker_details?;
                let amount = details.amount_display_string;
                (
                    MessageKind::Paid { amount },
                    details.super_sticker_metadata.alt_text,
                )
            }
            "newSponsorEvent" => {
                let details = snippet.new_sponsor_details?;
                let verb = if details.is_upgrade {
                    "Upgraded to"
                } else {
                    "Joined"
                };
                let level = match details.member_level_name.as_str() {
                    "" => "the membership",
                    name => name,
                };
                let headline = format!("{verb} {level}");
                (MessageKind::Membership { headline }, String::new())
            }
            "memberMilestoneChatEvent" => {
                let details = snippet.member_milestone_chat_details?;
                let headline = format!("Member for {} months", details.member_month);
                (MessageKind::Membership { headline }, details.user_comment)
            }
            "membershipGiftingEvent" => {
                let details = snippet.membership_gifting_details?;
                let headline = format!("Gifted {} memberships", details.gift_memberships_count);
                (MessageKind::Membership { headline }, String::new())
            }
            "messageDeletedEvent" => {
                return Some(ChatEvent::MessageDeleted {
                    message_id: snippet.message_deleted_details?.deleted_message_id,
                });
            }
            "userBannedEvent" => return snippet.user_banned_details?.into_event(),
            "chatEndedEvent" => return Some(ChatEvent::System("The live chat ended.".into())),
            _ => return None,
        };
        Some(ChatEvent::Message(ChatMessage {
            id: self.id,
            author: self.author_details?.into(),
            content,
            inline_emotes: Vec::new(),
            reply: None,
            kind,
            timestamp: snippet.published_at,
        }))
    }
}

impl UserBannedDetails {
    fn into_event(self) -> Option<ChatEvent> {
        let secs = self
            .ban_duration_seconds
            .and_then(|value| value.as_u64().or(value.as_str()?.parse().ok()));
        let user = self.banned_user_details;
        Some(ChatEvent::UserModerated(UserModeration {
            target: UserRef::new(Some(user.channel_id), Some(user.display_name))?,
            action: match (self.ban_type.as_str(), secs) {
                ("temporary", Some(secs)) => Moderation::Timeout { secs },
                _ => Moderation::Ban,
            },
            reason: None,
        }))
    }
}

/// The `error.errors[0].reason` of a failed call, such as `quotaExceeded`.
pub fn error_reason(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Body {
        error: ErrorBody,
    }
    #[derive(Deserialize)]
    struct ErrorBody {
        #[serde(default)]
        errors: Vec<ErrorItem>,
    }
    #[derive(Deserialize)]
    struct ErrorItem {
        reason: String,
    }
    let body: Body = serde_json::from_str(body).ok()?;
    body.error.errors.into_iter().next().map(|item| item.reason)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use time::macros::datetime;

    use super::*;

    fn event(kind: &str, details: Value) -> Option<ChatEvent> {
        let mut snippet = json!({
            "type": kind,
            "publishedAt": "2026-11-25T12:00:00.5Z",
            "displayMessage": "shown text",
        });
        snippet
            .as_object_mut()
            .unwrap()
            .extend(details.as_object().unwrap().clone());
        let item: LiveChatMessage = serde_json::from_value(json!({
            "id": "LCC.1",
            "snippet": snippet,
            "authorDetails": {
                "channelId": "UC1", "displayName": "@someone", "isVerified": false,
                "isChatOwner": false, "isChatSponsor": true, "isChatModerator": true
            }
        }))
        .unwrap();
        item.into_event()
    }

    fn message(kind: &str, details: Value) -> ChatMessage {
        match event(kind, details) {
            Some(ChatEvent::Message(message)) => message,
            other => panic!("not a message: {other:?}"),
        }
    }

    #[test]
    fn a_text_message_carries_author_and_badges() {
        let message = message("textMessageEvent", json!({}));
        assert_eq!(message.id, "LCC.1");
        assert_eq!(message.content, "shown text");
        assert_eq!(message.kind, MessageKind::Chat);
        assert_eq!(message.author.id, "UC1");
        assert_eq!(message.author.username, "someone");
        assert_eq!(message.author.role, Role::Moderator);
        assert_eq!(message.author.badges, ["moderator", "member"]);
        assert_eq!(message.timestamp, datetime!(2026-11-25 12:00:00.5 UTC));
    }

    #[test]
    fn paid_messages_keep_the_amount_and_the_comment() {
        let chat = message(
            "superChatEvent",
            json!({"superChatDetails": {"amountMicros": "5000000", "currency": "USD",
                "amountDisplayString": "$5.00", "userComment": "great stream", "tier": 2}}),
        );
        assert_eq!(
            chat.kind,
            MessageKind::Paid {
                amount: "$5.00".into()
            }
        );
        assert_eq!(chat.content, "great stream");

        let sticker = message(
            "superStickerEvent",
            json!({"superStickerDetails": {"amountDisplayString": "€2.00",
                "superStickerMetadata": {"altText": "A waving cat"}}}),
        );
        assert_eq!(sticker.content, "A waving cat");
    }

    #[test]
    fn memberships_become_headlines() {
        let headline = |kind, details| match message(kind, details).kind {
            MessageKind::Membership { headline } => headline,
            other => panic!("not a membership: {other:?}"),
        };
        assert_eq!(
            headline(
                "newSponsorEvent",
                json!({"newSponsorDetails": {"memberLevelName": "Gold", "isUpgrade": false}})
            ),
            "Joined Gold"
        );
        assert_eq!(
            headline(
                "memberMilestoneChatEvent",
                json!({"memberMilestoneChatDetails": {"memberMonth": 12, "userComment": "woo"}})
            ),
            "Member for 12 months"
        );
        assert_eq!(
            headline(
                "membershipGiftingEvent",
                json!({"membershipGiftingDetails": {"giftMembershipsCount": 5}})
            ),
            "Gifted 5 memberships"
        );
    }

    #[test]
    fn moderation_items_map_to_the_domain() {
        assert_eq!(
            event(
                "messageDeletedEvent",
                json!({"messageDeletedDetails": {"deletedMessageId": "LCC.0"}})
            ),
            Some(ChatEvent::MessageDeleted {
                message_id: "LCC.0".into()
            })
        );
        let user = json!({"channelId": "UC9", "displayName": "troll"});
        let moderated = |action| {
            Some(ChatEvent::UserModerated(UserModeration {
                target: UserRef::new(Some("UC9".into()), Some("troll".into())).unwrap(),
                action,
                reason: None,
            }))
        };
        assert_eq!(
            event(
                "userBannedEvent",
                json!({"userBannedDetails": {"bannedUserDetails": user,
                    "banType": "temporary", "banDurationSeconds": "300"}})
            ),
            moderated(Moderation::Timeout { secs: 300 })
        );
        assert_eq!(
            event(
                "userBannedEvent",
                json!({"userBannedDetails": {"bannedUserDetails": user, "banType": "permanent"}})
            ),
            moderated(Moderation::Ban)
        );
    }

    #[test]
    fn housekeeping_and_incomplete_items_are_skipped() {
        assert_eq!(event("sponsorOnlyModeStartedEvent", json!({})), None);
        assert_eq!(event("superChatEvent", json!({})), None);
    }

    #[test]
    fn reads_the_reason_of_an_api_error() {
        let body = r#"{"error":{"code":403,"errors":[{"reason":"quotaExceeded"}]}}"#;
        assert_eq!(error_reason(body).as_deref(), Some("quotaExceeded"));
        assert_eq!(error_reason("<html>"), None);
    }
}
