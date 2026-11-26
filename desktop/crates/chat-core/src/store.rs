use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use time::OffsetDateTime;

use crate::emotes::{EmoteRegistry, EmoteScope, tokenize_message};
use crate::event::{ChatEvent, ConnectionState, Identity, Moderation, RoomState, UserModeration};
use crate::message::{ChatMessage, Emote, MessageFragment, MessageKind};
use crate::platform::ChannelKey;
use crate::workspace::PaneMode;

const REPLY_EXCERPT_CHARS: usize = 200;

/// Why a message is struck out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    Deleted,
    TimedOut { secs: u64 },
    Banned,
}

/// Why a message stands out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Highlight {
    Mention,
    Keyword(String),
    /// Paid messages and memberships.
    Event,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoggedMessage {
    pub message: ChatMessage,
    pub fragments: Vec<MessageFragment>,
    pub removal: Option<Removal>,
    pub highlight: Option<Highlight>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntryBody {
    Message(LoggedMessage),
    Notice(String),
}

/// One line of a chat log. `seq` is unique and grows with arrival order.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub seq: u64,
    pub channel: ChannelKey,
    pub timestamp: OffsetDateTime,
    pub body: EntryBody,
}

impl LogEntry {
    pub fn message(&self) -> Option<&LoggedMessage> {
        match &self.body {
            EntryBody::Message(logged) => Some(logged),
            EntryBody::Notice(_) => None,
        }
    }

    fn order(&self) -> (OffsetDateTime, u64) {
        (self.timestamp, self.seq)
    }

    /// `needle` must already be lowercase.
    fn contains(&self, needle: &str) -> bool {
        let hit = |haystack: &str| haystack.to_lowercase().contains(needle);
        match &self.body {
            EntryBody::Notice(text) => hit(text),
            EntryBody::Message(logged) => {
                let message = &logged.message;
                hit(&message.content)
                    || hit(&message.author.username)
                    || hit(&message.author.display_name)
            }
        }
    }
}

/// How one pane looks at its feed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewState {
    pub search: String,
    paused_at: Option<u64>,
}

impl ViewState {
    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Freezes the view on what `store` holds now. Pausing again keeps the first freeze.
    pub fn pause(&mut self, store: &ChatStore) {
        self.paused_at.get_or_insert(store.next_seq);
    }

    pub fn resume(&mut self) {
        self.paused_at = None;
    }
}

#[derive(Debug, Default)]
pub struct ChannelState {
    log: VecDeque<Arc<LogEntry>>,
    message_ids: HashSet<String>,
    pub room: RoomState,
    pub connection: ConnectionState,
    pub identity: Option<Identity>,
}

impl ChannelState {
    pub fn can_moderate(&self) -> bool {
        self.identity
            .as_ref()
            .is_some_and(|identity| identity.role.can_moderate())
    }

    fn message(&self, id: &str) -> Option<&LoggedMessage> {
        self.log
            .iter()
            .rev()
            .filter_map(|entry| entry.message())
            .find(|logged| logged.message.id == id)
    }

    /// Keeps the log ordered by time, so replayed history lands where it belongs.
    fn insert(&mut self, entry: LogEntry, capacity: usize) {
        if let Some(logged) = entry.message() {
            self.message_ids.insert(logged.message.id.clone());
        }
        let at = self.log.partition_point(|e| e.order() <= entry.order());
        self.log.insert(at, Arc::new(entry));
        while self.log.len() > capacity {
            let evicted = self.log.pop_front();
            if let Some(logged) = evicted.as_ref().and_then(|entry| entry.message()) {
                self.message_ids.remove(&logged.message.id);
            }
        }
    }

    fn remove_where(&mut self, removal: Removal, matches: impl Fn(&ChatMessage) -> bool) {
        for entry in &mut self.log {
            let affected = entry
                .message()
                .is_some_and(|logged| logged.removal.is_none() && matches(&logged.message));
            if affected && let EntryBody::Message(logged) = &mut Arc::make_mut(entry).body {
                logged.removal = Some(removal);
            }
        }
    }

    fn retokenize(&mut self, emotes: &EmoteRegistry) {
        for entry in &mut self.log {
            let Some(logged) = entry.message() else {
                continue;
            };
            let message = &logged.message;
            let fragments = tokenize_message(
                &message.content,
                &message.inline_emotes,
                &entry.channel,
                emotes,
            );
            if fragments != logged.fragments
                && let EntryBody::Message(logged) = &mut Arc::make_mut(entry).body
            {
                logged.fragments = fragments;
            }
        }
    }
}

/// Every channel's log and state, changed only through [`ChatStore::apply`].
#[derive(Debug)]
pub struct ChatStore {
    channels: HashMap<ChannelKey, ChannelState>,
    emotes: EmoteRegistry,
    keywords: Vec<String>,
    capacity: usize,
    next_seq: u64,
    revision: u64,
}

impl ChatStore {
    pub fn new(capacity_per_channel: usize, highlight_keywords: &[String]) -> Self {
        Self {
            channels: HashMap::new(),
            emotes: EmoteRegistry::default(),
            keywords: highlight_keywords
                .iter()
                .map(|keyword| keyword.trim().to_lowercase())
                .filter(|keyword| !keyword.is_empty())
                .collect(),
            capacity: capacity_per_channel.max(1),
            next_seq: 0,
            revision: 0,
        }
    }

    /// Grows with every change, so a view can tell when to refresh.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn channel(&self, key: &ChannelKey) -> Option<&ChannelState> {
        self.channels.get(key)
    }

    pub fn forget(&mut self, key: &ChannelKey) {
        self.channels.remove(key);
        self.revision += 1;
    }

    pub fn install_global_emotes(&mut self, emotes: Vec<Emote>) {
        self.emotes.install(EmoteScope::Global, emotes);
        for channel in self.channels.values_mut() {
            channel.retokenize(&self.emotes);
        }
        self.revision += 1;
    }

    pub fn apply(&mut self, key: &ChannelKey, event: ChatEvent, now: OffsetDateTime) {
        self.revision += 1;
        match event {
            ChatEvent::Message(message) => self.add_message(key, message),
            ChatEvent::MessageDeleted { message_id } => self
                .state(key)
                .remove_where(Removal::Deleted, |message| message.id == message_id),
            ChatEvent::UserModerated(moderation) => self.moderate(key, moderation, now),
            ChatEvent::ChatCleared => {
                let state = self.state(key);
                state.log.clear();
                state.message_ids.clear();
                self.notice(key, "Chat was cleared by a moderator.".into(), now);
            }
            ChatEvent::RoomState(update) => self.state(key).room.merge(update),
            ChatEvent::ConnectionState(connection) => {
                let notice = match &connection {
                    ConnectionState::Connected => Some("Connected.".to_owned()),
                    ConnectionState::Error(reason) => Some(reason.clone()),
                    _ => None,
                };
                self.state(key).connection = connection;
                if let Some(text) = notice {
                    self.notice(key, text, now);
                }
            }
            ChatEvent::Emotes(emotes) => {
                self.emotes
                    .install(EmoteScope::Channel(key.clone()), emotes);
                self.channels
                    .entry(key.clone())
                    .or_default()
                    .retokenize(&self.emotes);
            }
            ChatEvent::Identity(identity) => self.state(key).identity = Some(identity),
            ChatEvent::System(text) => self.notice(key, text, now),
        }
    }

    /// The lines a pane shows: its channels merged by time, searched, and frozen if paused.
    pub fn rows(&self, mode: &PaneMode, view: &ViewState) -> Vec<Arc<LogEntry>> {
        let mut rows: Vec<_> = self
            .matching(mode, view)
            .filter(|entry| view.paused_at.is_none_or(|limit| entry.seq < limit))
            .cloned()
            .collect();
        rows.sort_by_key(|entry| entry.order());
        rows
    }

    /// Messages that arrived since the pane was paused and would be shown in it.
    pub fn unread(&self, mode: &PaneMode, view: &ViewState) -> usize {
        let Some(limit) = view.paused_at else {
            return 0;
        };
        self.matching(mode, view)
            .filter(|entry| entry.seq >= limit && entry.message().is_some())
            .count()
    }

    fn matching<'a>(
        &'a self,
        mode: &'a PaneMode,
        view: &ViewState,
    ) -> impl Iterator<Item = &'a Arc<LogEntry>> {
        let needle = view.search.trim().to_lowercase();
        mode.channels()
            .iter()
            .filter_map(|key| self.channels.get(key))
            .flat_map(|state| &state.log)
            .filter(move |entry| needle.is_empty() || entry.contains(&needle))
    }

    fn state(&mut self, key: &ChannelKey) -> &mut ChannelState {
        self.channels.entry(key.clone()).or_default()
    }

    fn push(&mut self, key: &ChannelKey, timestamp: OffsetDateTime, body: EntryBody) {
        let seq = self.next_seq;
        self.next_seq += 1;
        let entry = LogEntry {
            seq,
            channel: key.clone(),
            timestamp,
            body,
        };
        let capacity = self.capacity;
        self.state(key).insert(entry, capacity);
    }

    fn notice(&mut self, key: &ChannelKey, text: String, now: OffsetDateTime) {
        self.push(key, now, EntryBody::Notice(text));
    }

    fn add_message(&mut self, key: &ChannelKey, mut message: ChatMessage) {
        let state = self.channels.entry(key.clone()).or_default();
        // Reconnecting replays recent history.
        if state.message_ids.contains(&message.id) {
            return;
        }
        if let Some(reply) = &mut message.reply
            && let Some(parent) = state.message(&reply.message_id)
        {
            let parent = &parent.message;
            reply
                .author
                .get_or_insert_with(|| parent.author.display_name.clone());
            reply
                .excerpt
                .get_or_insert_with(|| parent.content.chars().take(REPLY_EXCERPT_CHARS).collect());
        }
        let logged = LoggedMessage {
            fragments: tokenize_message(
                &message.content,
                &message.inline_emotes,
                key,
                &self.emotes,
            ),
            highlight: highlight(&message, state.identity.as_ref(), &self.keywords),
            removal: None,
            message,
        };
        self.push(key, logged.message.timestamp, EntryBody::Message(logged));
    }

    fn moderate(&mut self, key: &ChannelKey, moderation: UserModeration, now: OffsetDateTime) {
        let UserModeration {
            target,
            action,
            reason,
        } = moderation;
        let (removal, outcome) = match action {
            Moderation::Timeout { secs } => (
                Some(Removal::TimedOut { secs }),
                format!("was timed out for {secs}s"),
            ),
            Moderation::Ban => (Some(Removal::Banned), "was banned".to_owned()),
            Moderation::Unban => (None, "was unbanned".to_owned()),
        };
        let state = self.state(key);
        let name = state
            .log
            .iter()
            .rev()
            .filter_map(|entry| entry.message())
            .find(|logged| target.matches(&logged.message.author))
            .map_or(target.label().to_owned(), |logged| {
                logged.message.author.display_name.clone()
            });
        if let Some(removal) = removal {
            state.remove_where(removal, |message| target.matches(&message.author));
        }
        let reason = reason
            .filter(|reason| !reason.is_empty())
            .map_or(String::new(), |reason| format!(": {reason}"));
        self.notice(key, format!("{name} {outcome}{reason}."), now);
    }
}

fn highlight(
    message: &ChatMessage,
    identity: Option<&Identity>,
    keywords: &[String],
) -> Option<Highlight> {
    if matches!(
        message.kind,
        MessageKind::Paid { .. } | MessageKind::Membership { .. }
    ) {
        return Some(Highlight::Event);
    }
    let own = identity.is_some_and(|me| me.username.eq_ignore_ascii_case(&message.author.username));
    if own {
        return None;
    }
    let content = message.content.to_lowercase();
    let mentioned = identity.is_some_and(|me| {
        contains_word(&content, &me.username.to_lowercase())
            || contains_word(&content, &me.display_name.to_lowercase())
    });
    if mentioned {
        return Some(Highlight::Mention);
    }
    keywords
        .iter()
        .find(|keyword| contains_word(&content, keyword))
        .cloned()
        .map(Highlight::Keyword)
}

/// Both sides lowercase. A match may not run into a letter or digit on either side.
fn contains_word(haystack: &str, word: &str) -> bool {
    !word.is_empty()
        && haystack.match_indices(word).any(|(start, found)| {
            let before = haystack[..start].chars().next_back();
            let after = haystack[start + found.len()..].chars().next();
            let inside_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            !inside_word(before) && !inside_word(after)
        })
}

#[cfg(test)]
mod tests {
    use time::Duration;
    use time::macros::datetime;

    use super::*;
    use crate::event::UserRef;
    use crate::message::{Author, Color, EmoteProvider, ReplyRef};
    use crate::platform::Role;

    const T0: OffsetDateTime = datetime!(2026-11-25 12:00 UTC);

    fn key(s: &str) -> ChannelKey {
        s.parse().unwrap()
    }

    fn at(secs: i64) -> OffsetDateTime {
        T0 + Duration::seconds(secs)
    }

    fn message(id: &str, user: &str, content: &str, secs: i64) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            author: Author {
                id: format!("id-{user}"),
                username: user.into(),
                display_name: user.to_uppercase(),
                color: Color(0xffffff),
                badges: vec![],
                role: Role::Member,
            },
            content: content.into(),
            inline_emotes: vec![],
            reply: None,
            kind: MessageKind::Chat,
            timestamp: at(secs),
        }
    }

    struct Fixture {
        store: ChatStore,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with(100, &[])
        }

        fn with(capacity: usize, keywords: &[&str]) -> Self {
            let keywords: Vec<String> = keywords.iter().map(|k| k.to_string()).collect();
            Self {
                store: ChatStore::new(capacity, &keywords),
            }
        }

        fn apply(&mut self, channel: &str, event: ChatEvent) {
            self.store.apply(&key(channel), event, at(1000));
        }

        fn say(&mut self, channel: &str, id: &str, user: &str, content: &str, secs: i64) {
            self.apply(
                channel,
                ChatEvent::Message(message(id, user, content, secs)),
            );
        }

        fn rows(&self, mode: &PaneMode, view: &ViewState) -> Vec<String> {
            self.store
                .rows(mode, view)
                .iter()
                .map(|entry| match &entry.body {
                    EntryBody::Message(logged) => logged.message.content.clone(),
                    EntryBody::Notice(text) => format!("* {text}"),
                })
                .collect()
        }

        fn single(&self, channel: &str) -> Vec<String> {
            self.rows(&PaneMode::Single(key(channel)), &ViewState::default())
        }

        fn logged(&self, channel: &str, id: &str) -> LoggedMessage {
            self.store
                .channel(&key(channel))
                .and_then(|state| state.message(id))
                .unwrap()
                .clone()
        }
    }

    fn merged(keys: &[&str]) -> PaneMode {
        PaneMode::Merged(keys.iter().map(|k| key(k)).collect())
    }

    #[test]
    fn merged_feed_interleaves_channels_by_time() {
        let mut f = Fixture::new();
        f.say("native:main", "n1", "a", "native one", 1);
        f.say("native:main", "n2", "a", "native four", 4);
        f.say("twitch:t", "t1", "b", "twitch two", 2);
        f.say("kick:k", "k1", "c", "kick three", 3);
        f.say("youtube:y", "y1", "d", "not shown", 3);

        let view = ViewState::default();
        assert_eq!(
            f.rows(&merged(&["native:main", "twitch:t", "kick:k"]), &view),
            ["native one", "twitch two", "kick three", "native four"]
        );
        assert_eq!(f.single("twitch:t"), ["twitch two"]);
    }

    #[test]
    fn same_instant_keeps_arrival_order() {
        let mut f = Fixture::new();
        f.say("twitch:t", "1", "a", "first", 5);
        f.say("kick:k", "2", "a", "second", 5);
        f.say("twitch:t", "3", "a", "third", 5);
        assert_eq!(
            f.rows(&merged(&["kick:k", "twitch:t"]), &ViewState::default()),
            ["first", "second", "third"]
        );
    }

    #[test]
    fn replayed_history_is_deduplicated_and_slots_into_place() {
        let mut f = Fixture::new();
        f.say("native:main", "1", "a", "one", 1);
        f.say("native:main", "3", "a", "three", 3);
        for (id, text, secs) in [("1", "one", 1), ("2", "two", 2), ("3", "three", 3)] {
            f.say("native:main", id, "a", text, secs);
        }
        assert_eq!(f.single("native:main"), ["one", "two", "three"]);
    }

    #[test]
    fn ring_buffer_drops_the_oldest_per_channel() {
        let mut f = Fixture::with(2, &[]);
        for n in 1..=3 {
            f.say("twitch:t", &n.to_string(), "a", &format!("m{n}"), n);
        }
        f.say("kick:k", "k", "a", "other", 1);
        assert_eq!(f.single("twitch:t"), ["m2", "m3"]);
        assert_eq!(f.single("kick:k"), ["other"]);
    }

    #[test]
    fn deleting_strikes_only_that_message_in_that_channel() {
        let mut f = Fixture::new();
        f.say("native:main", "1", "a", "bad", 1);
        f.say("native:main", "2", "a", "fine", 2);
        f.say("twitch:t", "1", "a", "same id elsewhere", 1);
        f.apply(
            "native:main",
            ChatEvent::MessageDeleted {
                message_id: "1".into(),
            },
        );
        assert_eq!(f.logged("native:main", "1").removal, Some(Removal::Deleted));
        assert_eq!(f.logged("native:main", "2").removal, None);
        assert_eq!(f.logged("twitch:t", "1").removal, None);
    }

    fn moderation(target: UserRef, action: Moderation) -> ChatEvent {
        ChatEvent::UserModerated(UserModeration {
            target,
            action,
            reason: Some("spam".into()),
        })
    }

    #[test]
    fn timeout_strikes_every_message_by_the_target_and_says_so() {
        let mut f = Fixture::new();
        f.say("native:main", "1", "troll", "x", 1);
        f.say("native:main", "2", "other", "y", 2);
        f.say("native:main", "3", "troll", "z", 3);
        let target = UserRef::new(None, Some("Troll".into())).unwrap();
        f.apply(
            "native:main",
            moderation(target, Moderation::Timeout { secs: 60 }),
        );

        let removed = Some(Removal::TimedOut { secs: 60 });
        assert_eq!(f.logged("native:main", "1").removal, removed);
        assert_eq!(f.logged("native:main", "2").removal, None);
        assert_eq!(f.logged("native:main", "3").removal, removed);
        assert_eq!(
            f.single("native:main").last().unwrap(),
            "* TROLL was timed out for 60s: spam."
        );
    }

    #[test]
    fn ban_by_id_spares_a_namesake_and_unban_restores_nothing() {
        let mut f = Fixture::new();
        f.say("native:main", "1", "guest", "bad guest", 1);
        let mut namesake = message("2", "guest", "good guest", 2);
        namesake.author.id = "id-other".into();
        f.apply("native:main", ChatEvent::Message(namesake));

        let target = UserRef::new(Some("id-guest".into()), Some("guest".into())).unwrap();
        f.apply("native:main", moderation(target.clone(), Moderation::Ban));
        f.apply("native:main", moderation(target, Moderation::Unban));
        assert_eq!(f.logged("native:main", "1").removal, Some(Removal::Banned));
        assert_eq!(f.logged("native:main", "2").removal, None);
    }

    #[test]
    fn clearing_empties_one_channel_and_leaves_a_notice() {
        let mut f = Fixture::new();
        f.say("native:main", "1", "a", "gone", 1);
        f.say("twitch:t", "1", "a", "stays", 1);
        f.apply("native:main", ChatEvent::ChatCleared);
        assert_eq!(
            f.single("native:main"),
            ["* Chat was cleared by a moderator."]
        );
        assert_eq!(f.single("twitch:t"), ["stays"]);
    }

    #[test]
    fn room_state_updates_merge() {
        let mut f = Fixture::new();
        f.apply(
            "twitch:t",
            ChatEvent::RoomState(RoomState {
                slow_mode_secs: Some(30),
                viewer_count: Some(5),
                ..RoomState::default()
            }),
        );
        f.apply(
            "twitch:t",
            ChatEvent::RoomState(RoomState {
                viewer_count: Some(6),
                ..RoomState::default()
            }),
        );
        let room = &f.store.channel(&key("twitch:t")).unwrap().room;
        assert_eq!(room.slow_mode_secs, Some(30));
        assert_eq!(room.viewer_count, Some(6));
    }

    #[test]
    fn pausing_freezes_rows_and_counts_unread_messages() {
        let mut f = Fixture::new();
        let mode = merged(&["native:main", "twitch:t"]);
        f.say("native:main", "1", "a", "before", 1);
        let mut view = ViewState::default();
        view.pause(&f.store);

        f.say("native:main", "2", "a", "after", 2);
        f.say("twitch:t", "3", "a", "late but older", 0);
        f.say("kick:k", "4", "a", "another channel", 3);
        f.apply("native:main", ChatEvent::System("notice".into()));
        view.pause(&f.store);

        assert_eq!(f.rows(&mode, &view), ["before"]);
        assert_eq!(f.store.unread(&mode, &view), 2);

        view.resume();
        assert_eq!(f.store.unread(&mode, &view), 0);
        assert_eq!(
            f.rows(&mode, &view),
            ["late but older", "before", "after", "* notice"]
        );
    }

    #[test]
    fn a_deletion_still_shows_while_paused() {
        let mut f = Fixture::new();
        f.say("native:main", "1", "a", "bad", 1);
        let mut view = ViewState::default();
        view.pause(&f.store);
        f.apply(
            "native:main",
            ChatEvent::MessageDeleted {
                message_id: "1".into(),
            },
        );
        let rows = f.store.rows(&PaneMode::Single(key("native:main")), &view);
        assert_eq!(rows[0].message().unwrap().removal, Some(Removal::Deleted));
    }

    #[test]
    fn search_matches_content_and_author_in_any_case() {
        let mut f = Fixture::new();
        f.say("native:main", "1", "ferret", "hello", 1);
        f.say("native:main", "2", "otter", "HELLO there", 2);
        f.say("native:main", "3", "badger", "bye", 3);
        let search = |text: &str| {
            let view = ViewState {
                search: text.into(),
                ..ViewState::default()
            };
            f.rows(&PaneMode::Single(key("native:main")), &view)
        };
        assert_eq!(search("hello"), ["hello", "HELLO there"]);
        assert_eq!(search(" FERRET "), ["hello"]);
        assert_eq!(search("nothing"), [] as [&str; 0]);
        assert_eq!(search("").len(), 3);
    }

    #[test]
    fn highlights_mentions_and_keywords_on_word_boundaries() {
        let mut f = Fixture::with(100, &["Stream Deck", "rust"]);
        f.apply(
            "native:main",
            ChatEvent::Identity(Identity {
                username: "me".into(),
                display_name: "Me Too".into(),
                role: Role::Member,
            }),
        );
        for (id, user, text) in [
            ("mention", "a", "hi @Me!"),
            ("inside", "a", "some meme"),
            ("display", "a", "is me too here"),
            ("phrase", "a", "new STREAM DECK day"),
            ("prefix", "a", "rusty"),
            ("own", "me", "rust me"),
        ] {
            f.say("native:main", id, user, text, 1);
        }
        let got = |id: &str| f.logged("native:main", id).highlight;
        assert_eq!(got("mention"), Some(Highlight::Mention));
        assert_eq!(got("inside"), None);
        assert_eq!(got("display"), Some(Highlight::Mention));
        assert_eq!(
            got("phrase"),
            Some(Highlight::Keyword("stream deck".into()))
        );
        assert_eq!(got("prefix"), None);
        assert_eq!(got("own"), None);
    }

    #[test]
    fn paid_messages_are_highlighted() {
        let mut f = Fixture::new();
        let mut paid = message("1", "a", "take my money", 1);
        paid.kind = MessageKind::Paid {
            amount: "$5.00".into(),
        };
        f.apply("youtube:y", ChatEvent::Message(paid));
        assert_eq!(f.logged("youtube:y", "1").highlight, Some(Highlight::Event));
    }

    #[test]
    fn a_reply_by_id_picks_up_its_parent() {
        let mut f = Fixture::new();
        f.say("native:main", "1", "otter", "the question", 1);
        let mut reply = message("2", "a", "the answer", 2);
        reply.reply = Some(ReplyRef {
            message_id: "1".into(),
            author: None,
            excerpt: None,
        });
        f.apply("native:main", ChatEvent::Message(reply));
        let reply = f.logged("native:main", "2").message.reply.unwrap();
        assert_eq!(reply.author.as_deref(), Some("OTTER"));
        assert_eq!(reply.excerpt.as_deref(), Some("the question"));
    }

    #[test]
    fn emotes_that_load_late_apply_to_earlier_messages() {
        let mut f = Fixture::new();
        let emote = |code: &str| Emote {
            code: code.into(),
            url: "https://cdn.example/x.webp".into(),
            animated: false,
            provider: EmoteProvider::Native,
        };
        f.say("native:main", "1", "a", "wave", 1);
        f.say("twitch:t", "2", "a", "wave party", 1);
        f.apply("native:main", ChatEvent::Emotes(vec![emote("wave")]));
        f.store.install_global_emotes(vec![emote("party")]);

        let is_emote = |fragment: &MessageFragment| matches!(fragment, MessageFragment::Emote(_));
        assert!(is_emote(&f.logged("native:main", "1").fragments[0]));
        let twitch = f.logged("twitch:t", "2").fragments;
        assert_eq!(twitch[0], MessageFragment::Text("wave ".into()));
        assert!(is_emote(&twitch[1]));
    }

    #[test]
    fn connection_errors_are_kept_and_written_to_the_log() {
        let mut f = Fixture::new();
        let error = ConnectionState::Error("bad token".into());
        f.apply("native:main", ChatEvent::ConnectionState(error.clone()));
        let state = f.store.channel(&key("native:main")).unwrap();
        assert_eq!(state.connection, error);
        assert_eq!(f.single("native:main"), ["* bad token"]);
    }

    #[test]
    fn moderation_rights_follow_the_identity() {
        let mut f = Fixture::new();
        for (role, expected) in [(Role::Member, false), (Role::Moderator, true)] {
            f.apply(
                "native:main",
                ChatEvent::Identity(Identity {
                    username: "me".into(),
                    display_name: "Me".into(),
                    role,
                }),
            );
            let state = f.store.channel(&key("native:main")).unwrap();
            assert_eq!(state.can_moderate(), expected);
        }
    }
}
