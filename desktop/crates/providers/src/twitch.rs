mod irc;

use std::collections::VecDeque;
use std::time::Duration;

use chat_core::{
    Author, ChannelKey, ChatEvent, ChatMessage, Color, Emote, EmoteProvider, Identity, InlineEmote,
    MessageKind, Moderation, ReplyRef, Role, RoomState, UserModeration, UserRef,
};
use futures::future::BoxFuture;
use time::OffsetDateTime;

use crate::config::{ProviderConfig, TwitchAuth};
use crate::provider::{ChatProvider, ProviderCommand, SessionError, SessionIo};
use crate::socket::{Socket, dropped};
use crate::third_party;
use irc::{IrcMessage, parse_line};

const PING_EVERY: Duration = Duration::from_secs(60);
const IDLE_LIMIT: Duration = Duration::from_secs(150);
const ANONYMOUS_NICK: &str = "justinfan64218";

/// Twitch IRC over a websocket. Anonymous sessions can only read.
pub struct TwitchProvider {
    channel: ChannelKey,
    config: ProviderConfig,
    emotes_requested: bool,
}

/// What a session remembers about this client.
#[derive(Default)]
struct SelfState {
    login: String,
    /// Sent but not yet confirmed. Twitch never echoes our own messages.
    pending: VecDeque<PendingMessage>,
}

struct PendingMessage {
    content: String,
    reply_to: Option<String>,
}

impl TwitchProvider {
    pub fn new(channel: ChannelKey, config: &ProviderConfig) -> Self {
        Self {
            channel,
            config: config.clone(),
            emotes_requested: false,
        }
    }

    async fn session(&mut self, io: &mut SessionIo) -> Result<(), SessionError> {
        let mut socket = Socket::connect(&self.config.endpoints.twitch_irc, IDLE_LIMIT).await?;
        socket
            .send("CAP REQ :twitch.tv/tags twitch.tv/commands")
            .await?;
        let nick = match &self.config.twitch {
            TwitchAuth::Anonymous => ANONYMOUS_NICK,
            TwitchAuth::OAuth { login, token } => {
                socket.send(format!("PASS oauth:{token}")).await?;
                login
            }
        };
        socket.send(format!("NICK {nick}")).await?;

        let mut me = SelfState::default();
        let mut ping = tokio::time::interval(PING_EVERY);
        loop {
            tokio::select! {
                frame = socket.next() => {
                    for line in frame?.lines() {
                        if let Some(message) = parse_line(line)
                            && let Some(reply) = self.receive(&message, &mut me, io)?
                        {
                            socket.send(reply).await?;
                        }
                    }
                }
                _ = ping.tick() => socket.send("PING :keepalive").await?,
                command = io.command() => match command {
                    None => return Ok(()),
                    Some(ProviderCommand::Moderate(_)) => {
                        io.system("Moderating Twitch from here is not supported.");
                    }
                    Some(ProviderCommand::Send { .. }) if !self.can_send() => {
                        io.system("Sign in to Twitch to chat here.");
                    }
                    Some(ProviderCommand::Send { content, reply_to }) => {
                        let message = PendingMessage {
                            content: single_line(&content),
                            reply_to,
                        };
                        socket.send(privmsg(self.channel.channel(), &message)).await?;
                        me.pending.push_back(message);
                    }
                },
            }
        }
    }

    /// Handles one line. Returns what to answer, if anything.
    fn receive(
        &mut self,
        message: &IrcMessage<'_>,
        me: &mut SelfState,
        io: &mut SessionIo,
    ) -> Result<Option<String>, SessionError> {
        match message.command {
            "PING" => {
                let token = message.param(0).unwrap_or_default();
                return Ok(Some(format!("PONG :{token}")));
            }
            "001" => {
                me.login = message.param(0).unwrap_or_default().to_owned();
                return Ok(Some(format!("JOIN #{}", self.channel.channel())));
            }
            "RECONNECT" => return Err(dropped("Twitch asked to reconnect")),
            "NOTICE" => self.notice(message, me, io)?,
            "USERSTATE" => self.user_state(message, me, io),
            "366" => io.connected(),
            "ROOMSTATE" => {
                io.connected();
                self.request_emotes(message.tag("room-id"), io);
                io.emit_all(translate(message));
            }
            _ => io.emit_all(translate(message)),
        }
        Ok(None)
    }

    fn notice(
        &self,
        message: &IrcMessage<'_>,
        me: &mut SelfState,
        io: &SessionIo,
    ) -> Result<(), SessionError> {
        let text = message.param(1).unwrap_or_default();
        // Sign-in failures arrive before the welcome, with no message id.
        if me.login.is_empty() {
            return Err(SessionError::Fatal(format!(
                "Twitch sign-in failed: {text}"
            )));
        }
        if message
            .tag("msg-id")
            .is_some_and(|id| id.starts_with("msg_"))
        {
            me.pending.pop_front();
        }
        io.system(text);
        Ok(())
    }

    fn user_state(&self, message: &IrcMessage<'_>, me: &mut SelfState, io: &SessionIo) {
        let author = author(message, &me.login);
        io.emit(ChatEvent::Identity(Identity {
            username: author.username.clone(),
            display_name: author.display_name.clone(),
            role: author.role,
        }));
        if let Some(sent) = me.pending.pop_front() {
            let now = OffsetDateTime::now_utc();
            let id = message.tag("id").map_or_else(
                || format!("local-{}", now.unix_timestamp_nanos()),
                str::to_owned,
            );
            io.emit(ChatEvent::Message(ChatMessage {
                id,
                author,
                content: sent.content,
                inline_emotes: Vec::new(),
                reply: sent.reply_to.map(|message_id| ReplyRef {
                    message_id,
                    author: None,
                    excerpt: None,
                }),
                kind: MessageKind::Chat,
                timestamp: now,
            }));
        }
    }

    fn request_emotes(&mut self, room_id: Option<&str>, io: &SessionIo) {
        let Some(room_id) = room_id else { return };
        if !self.config.third_party_emotes || std::mem::replace(&mut self.emotes_requested, true) {
            return;
        }
        let http = self.config.http.clone();
        let room_id = room_id.to_owned();
        io.load_emotes(async move { third_party::fetch_twitch_channel(&http, &room_id).await });
    }
}

impl ChatProvider for TwitchProvider {
    fn channel(&self) -> &ChannelKey {
        &self.channel
    }

    fn can_send(&self) -> bool {
        matches!(self.config.twitch, TwitchAuth::OAuth { .. })
    }

    fn run_session<'a>(
        &'a mut self,
        io: &'a mut SessionIo,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(self.session(io))
    }
}

/// A line break would start a new IRC command.
fn single_line(text: &str) -> String {
    text.split(['\r', '\n']).collect::<Vec<_>>().join(" ")
}

fn privmsg(channel: &str, message: &PendingMessage) -> String {
    let tag = match &message.reply_to {
        Some(id) => format!("@reply-parent-msg-id={id} "),
        None => String::new(),
    };
    format!("{tag}PRIVMSG #{channel} :{}", message.content)
}

/// What a channel line means. Session upkeep is not here.
fn translate(message: &IrcMessage<'_>) -> Vec<ChatEvent> {
    match message.command {
        "PRIVMSG" => private_message(message).into_iter().collect(),
        "USERNOTICE" => user_notice(message).into_iter().collect(),
        "CLEARMSG" => message
            .tag("target-msg-id")
            .map(|id| ChatEvent::MessageDeleted {
                message_id: id.to_owned(),
            })
            .into_iter()
            .collect(),
        "CLEARCHAT" => vec![clear_chat(message)],
        "ROOMSTATE" => vec![ChatEvent::RoomState(RoomState {
            slow_mode_secs: message.tag("slow").and_then(|secs| secs.parse().ok()),
            ..RoomState::default()
        })],
        _ => Vec::new(),
    }
}

fn private_message(message: &IrcMessage<'_>) -> Option<ChatEvent> {
    let text = message.param(1)?;
    let (kind, text) = match text
        .strip_prefix("\u{1}ACTION ")
        .map(|text| text.trim_end_matches('\u{1}'))
    {
        Some(action) => (MessageKind::Action, action),
        None => (MessageKind::Chat, text),
    };
    let kind = match message.tag("bits") {
        Some(bits) => MessageKind::Paid {
            amount: format!("{bits} bits"),
        },
        None => kind,
    };
    chat_message(message, text, kind).map(ChatEvent::Message)
}

fn user_notice(message: &IrcMessage<'_>) -> Option<ChatEvent> {
    let headline = message.tag("system-msg").unwrap_or_default().to_owned();
    match message.param(1) {
        Some(text) => {
            let kind = MessageKind::Membership { headline };
            chat_message(message, text, kind).map(ChatEvent::Message)
        }
        None => (!headline.is_empty()).then_some(ChatEvent::System(headline)),
    }
}

fn chat_message(message: &IrcMessage<'_>, text: &str, kind: MessageKind) -> Option<ChatMessage> {
    let login = message.nick.or(message.tag("login"))?;
    Some(ChatMessage {
        id: message.tag("id")?.to_owned(),
        author: author(message, login),
        content: text.to_owned(),
        inline_emotes: message
            .tag("emotes")
            .map(|tag| inline_emotes(tag, text))
            .unwrap_or_default(),
        reply: message.tag("reply-parent-msg-id").map(|id| ReplyRef {
            message_id: id.to_owned(),
            author: message.tag("reply-parent-display-name").map(str::to_owned),
            excerpt: message.tag("reply-parent-msg-body").map(str::to_owned),
        }),
        kind,
        timestamp: message
            .tag("tmi-sent-ts")
            .and_then(|millis| millis.parse::<i128>().ok())
            .and_then(|millis| OffsetDateTime::from_unix_timestamp_nanos(millis * 1_000_000).ok())
            .unwrap_or_else(OffsetDateTime::now_utc),
    })
}

fn author(message: &IrcMessage<'_>, login: &str) -> Author {
    // `subscriber/12` carries a version that means nothing to a reader.
    let badges: Vec<String> = message
        .tag("badges")
        .unwrap_or_default()
        .split(',')
        .filter_map(|badge| badge.split('/').next())
        .filter(|badge| !badge.is_empty())
        .map(str::to_owned)
        .collect();
    let has = |name: &str| badges.iter().any(|badge| badge == name);
    let role = if has("broadcaster") {
        Role::Broadcaster
    } else if has("moderator") || message.tag("mod") == Some("1") {
        Role::Moderator
    } else if has("vip") {
        Role::Vip
    } else {
        Role::Member
    };
    Author {
        id: message.tag("user-id").unwrap_or_default().to_owned(),
        username: login.to_owned(),
        display_name: message.tag("display-name").unwrap_or(login).to_owned(),
        color: message
            .tag("color")
            .and_then(Color::parse)
            .unwrap_or_else(|| Color::for_name(login)),
        badges,
        role,
    }
}

/// Parses `id:first-last,first-last/id:...`, whose positions count characters inclusively.
fn inline_emotes(tag: &str, text: &str) -> Vec<InlineEmote> {
    let byte_at = |index: usize| text.char_indices().nth(index);
    tag.split('/')
        .filter_map(|group| group.split_once(':'))
        .flat_map(|(id, positions)| positions.split(',').map(move |position| (id, position)))
        .filter_map(|(id, position)| {
            let (first, last) = position.split_once('-')?;
            let (start, _) = byte_at(first.parse().ok()?)?;
            let (end, last_char) = byte_at(last.parse().ok()?)?;
            let range = start..end + last_char.len_utf8();
            Some(InlineEmote {
                emote: Emote {
                    code: text.get(range.clone())?.to_owned(),
                    url: format!("https://static-cdn.jtvnw.net/emoticons/v2/{id}/default/dark/2.0"),
                    animated: id.starts_with("emotesv2_"),
                    provider: EmoteProvider::Twitch,
                },
                range,
            })
        })
        .collect()
}

fn clear_chat(message: &IrcMessage<'_>) -> ChatEvent {
    let target = message.tag("target-user-id").map(str::to_owned);
    let name = message.param(1).map(str::to_owned);
    match UserRef::new(target, name) {
        None => ChatEvent::ChatCleared,
        Some(target) => ChatEvent::UserModerated(UserModeration {
            target,
            action: match message.tag("ban-duration").and_then(|s| s.parse().ok()) {
                Some(secs) => Moderation::Timeout { secs },
                None => Moderation::Ban,
            },
            reason: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn events(line: &str) -> Vec<ChatEvent> {
        translate(&parse_line(line).unwrap())
    }

    fn message(line: &str) -> ChatMessage {
        match events(line).remove(0) {
            ChatEvent::Message(message) => message,
            other => panic!("not a message: {other:?}"),
        }
    }

    const PRIVMSG: &str = "@badge-info=subscriber/14;badges=moderator/1,subscriber/12;\
        color=#1E90FF;display-name=SomeMod;emotes=;id=abc-123;mod=1;room-id=1;\
        tmi-sent-ts=1764072000123;user-id=42 \
        :somemod!somemod@somemod.tmi.twitch.tv PRIVMSG #chan :hello there";

    #[test]
    fn a_privmsg_carries_author_badges_color_and_time() {
        let message = message(PRIVMSG);
        assert_eq!(message.id, "abc-123");
        assert_eq!(message.content, "hello there");
        assert_eq!(message.author.id, "42");
        assert_eq!(message.author.username, "somemod");
        assert_eq!(message.author.display_name, "SomeMod");
        assert_eq!(message.author.color, Color(0x1e90ff));
        assert_eq!(message.author.badges, ["moderator", "subscriber"]);
        assert_eq!(message.author.role, Role::Moderator);
        assert_eq!(message.timestamp, datetime!(2025-11-25 12:00:00.123 UTC));
        assert_eq!(message.kind, MessageKind::Chat);
        assert_eq!(message.reply, None);
    }

    #[test]
    fn a_reply_names_its_parent() {
        let message = message(
            "@id=2;display-name=A;reply-parent-msg-id=parent-1;\
             reply-parent-display-name=Other;reply-parent-msg-body=the\\squestion \
             :a!a@a.tmi.twitch.tv PRIVMSG #chan :@Other yes",
        );
        assert_eq!(
            message.reply,
            Some(ReplyRef {
                message_id: "parent-1".into(),
                author: Some("Other".into()),
                excerpt: Some("the question".into()),
            })
        );
    }

    #[test]
    fn emote_positions_count_characters_not_bytes() {
        let message = message(
            "@id=1;emotes=25:2-6,12-16/emotesv2_x:8-10 \
             :a!a@a.tmi.twitch.tv PRIVMSG #chan :é Kappa abc Kappa",
        );
        let found: Vec<_> = message
            .inline_emotes
            .iter()
            .map(|placed| {
                (
                    &message.content[placed.range.clone()],
                    placed.emote.code.as_str(),
                    placed.emote.animated,
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                ("Kappa", "Kappa", false),
                ("Kappa", "Kappa", false),
                ("abc", "abc", true)
            ]
        );
    }

    #[test]
    fn malformed_emote_tags_are_dropped() {
        for tag in ["25", "25:", "25:a-b", "25:5", "25:90-99", "25:4-2"] {
            assert_eq!(inline_emotes(tag, "Kappa"), [], "{tag}");
        }
    }

    #[test]
    fn actions_and_cheers_are_told_apart() {
        let action = message("@id=1 :a!a@a.tmi.twitch.tv PRIVMSG #chan :\u{1}ACTION waves\u{1}");
        assert_eq!(action.kind, MessageKind::Action);
        assert_eq!(action.content, "waves");

        let cheer = message("@id=2;bits=100 :a!a@a.tmi.twitch.tv PRIVMSG #chan :Cheer100");
        assert_eq!(
            cheer.kind,
            MessageKind::Paid {
                amount: "100 bits".into()
            }
        );
    }

    #[test]
    fn clearchat_means_timeout_ban_or_clear() {
        let moderated = |action| {
            vec![ChatEvent::UserModerated(UserModeration {
                target: UserRef::new(Some("7".into()), Some("troll".into())).unwrap(),
                action,
                reason: None,
            })]
        };
        assert_eq!(
            events("@ban-duration=60;target-user-id=7 :tmi.twitch.tv CLEARCHAT #chan :troll"),
            moderated(Moderation::Timeout { secs: 60 })
        );
        assert_eq!(
            events("@target-user-id=7 :tmi.twitch.tv CLEARCHAT #chan :troll"),
            moderated(Moderation::Ban)
        );
        assert_eq!(
            events("@room-id=1 :tmi.twitch.tv CLEARCHAT #chan"),
            [ChatEvent::ChatCleared]
        );
    }

    #[test]
    fn clearmsg_deletes_one_message() {
        assert_eq!(
            events("@login=troll;target-msg-id=abc :tmi.twitch.tv CLEARMSG #chan :text"),
            [ChatEvent::MessageDeleted {
                message_id: "abc".into()
            }]
        );
    }

    #[test]
    fn subscriptions_are_messages_or_notices() {
        let with_text = message(
            "@id=1;login=fan;display-name=Fan;system-msg=Fan\\ssubscribed! \
             :tmi.twitch.tv USERNOTICE #chan :love it",
        );
        assert_eq!(with_text.author.display_name, "Fan");
        assert_eq!(
            with_text.kind,
            MessageKind::Membership {
                headline: "Fan subscribed!".into()
            }
        );
        assert_eq!(
            events("@id=1;login=fan;system-msg=Fan\\ssubscribed! :tmi.twitch.tv USERNOTICE #chan"),
            [ChatEvent::System("Fan subscribed!".into())]
        );
    }

    #[test]
    fn sent_text_cannot_start_a_second_command() {
        let message = PendingMessage {
            content: single_line("hi\r\nPRIVMSG #other :gotcha"),
            reply_to: Some("abc".into()),
        };
        assert_eq!(
            privmsg("chan", &message),
            "@reply-parent-msg-id=abc PRIVMSG #chan :hi  PRIVMSG #other :gotcha"
        );
    }
}
