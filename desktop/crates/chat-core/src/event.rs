use crate::message::{Author, ChatMessage, Emote};
use crate::platform::Role;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting {
        attempt: u32,
    },
    /// Gave up. Retrying would not help without a change of credentials or channel.
    Error(String),
}

/// A partial update: `None` leaves the known value alone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoomState {
    pub title: Option<String>,
    pub slow_mode_secs: Option<u32>,
    pub guests_allowed: Option<bool>,
    pub viewer_count: Option<u64>,
}

impl RoomState {
    pub fn merge(&mut self, update: RoomState) {
        self.title = update.title.or(self.title.take());
        self.slow_mode_secs = update.slow_mode_secs.or(self.slow_mode_secs);
        self.guests_allowed = update.guests_allowed.or(self.guests_allowed);
        self.viewer_count = update.viewer_count.or(self.viewer_count);
    }
}

/// Who this client is in a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub username: String,
    pub display_name: String,
    pub role: Role,
}

/// A user named by id, by username, or both. Never neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRef {
    id: Option<String>,
    username: Option<String>,
}

impl UserRef {
    pub fn new(id: Option<String>, username: Option<String>) -> Option<Self> {
        let id = id.filter(|s| !s.is_empty());
        let username = username.filter(|s| !s.is_empty());
        (id.is_some() || username.is_some()).then_some(Self { id, username })
    }

    pub fn of(author: &Author) -> Self {
        Self {
            id: Some(author.id.clone()).filter(|id| !id.is_empty()),
            username: Some(author.username.clone()),
        }
    }

    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    /// The id decides when there is one, since guests may share a username.
    pub fn matches(&self, author: &Author) -> bool {
        match (&self.id, &self.username) {
            (Some(id), _) => *id == author.id,
            (None, Some(name)) => name.eq_ignore_ascii_case(&author.username),
            (None, None) => false,
        }
    }

    pub fn label(&self) -> &str {
        self.username
            .as_deref()
            .or(self.id.as_deref())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moderation {
    Timeout { secs: u64 },
    Ban,
    Unban,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserModeration {
    pub target: UserRef,
    pub action: Moderation,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEvent {
    Message(ChatMessage),
    MessageDeleted {
        message_id: String,
    },
    UserModerated(UserModeration),
    ChatCleared,
    RoomState(RoomState),
    ConnectionState(ConnectionState),
    /// Emotes usable in this channel. Adds to any already known.
    Emotes(Vec<Emote>),
    Identity(Identity),
    System(String),
}

/// A moderation request from this client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModAction {
    Delete {
        message_id: String,
    },
    Timeout {
        target: UserRef,
        secs: u64,
        reason: Option<String>,
    },
    Ban {
        target: UserRef,
        reason: Option<String>,
    },
    Unban {
        target: UserRef,
    },
    Clear,
    SetRole {
        target: UserRef,
        role: Role,
    },
}
