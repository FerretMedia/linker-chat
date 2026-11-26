//! Async chat connectors. Each one turns a platform's wire format into
//! [`chat_core::ChatEvent`]s and carries [`ProviderCommand`]s back.

mod config;
mod kick;
mod native;
mod provider;
mod socket;
mod third_party;
mod twitch;
mod youtube;

pub use config::{
    Endpoints, NativeAuth, NativeConfig, ProviderConfig, TwitchAuth, YouTubeCredentials,
};
pub use kick::KickProvider;
pub use native::NativeProvider;
pub use provider::{
    ChatProvider, EventSender, ProviderCommand, ProviderEvent, ProviderHandle, ReconnectPolicy,
    SessionError, SessionIo, spawn,
};
pub use third_party::fetch_global_emotes;
pub use twitch::TwitchProvider;
pub use youtube::YouTubeProvider;

use chat_core::{ChannelKey, Platform};

/// The connector for `channel`.
pub fn connect(channel: ChannelKey, config: &ProviderConfig) -> Box<dyn ChatProvider> {
    match channel.platform() {
        Platform::Native => Box::new(NativeProvider::new(channel, config)),
        Platform::Twitch => Box::new(TwitchProvider::new(channel, config)),
        Platform::Kick => Box::new(KickProvider::new(channel, config)),
        Platform::YouTube => Box::new(YouTubeProvider::new(channel, config)),
    }
}
