//! Platform-neutral chat domain: types, emote tokenizing, and the state reducer.
//!
//! Nothing here performs I/O or knows about a UI toolkit.

mod emotes;
mod event;
mod message;
mod platform;
mod rows;
mod store;
mod workspace;

pub use emotes::{EmoteRegistry, EmoteScope, tokenize_message};
pub use event::{
    ChatEvent, ConnectionState, Identity, ModAction, Moderation, RoomState, UserModeration, UserRef,
};
pub use message::{
    Author, ChatMessage, Color, Emote, EmoteProvider, InlineEmote, MessageFragment, MessageKind,
    ReplyRef,
};
pub use platform::{ChannelKey, InvalidChannel, Platform, Role};
pub use rows::{Splice, diff_rows};
pub use store::{
    ChannelState, ChatStore, EntryBody, Highlight, LogEntry, LoggedMessage, Removal, ViewState,
};
pub use workspace::{PaneMode, SplitPane, TabLayout, WorkspaceConfig};
