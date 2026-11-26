use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Native,
    Twitch,
    YouTube,
    Kick,
}

impl Platform {
    pub const ALL: [Platform; 4] = [
        Platform::Native,
        Platform::Twitch,
        Platform::Kick,
        Platform::YouTube,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Platform::Native => "Native",
            Platform::Twitch => "Twitch",
            Platform::YouTube => "YouTube",
            Platform::Kick => "Kick",
        }
    }

    /// Short tag shown on platform pills.
    pub fn tag(self) -> &'static str {
        match self {
            Platform::Native => "NAT",
            Platform::Twitch => "TW",
            Platform::YouTube => "YT",
            Platform::Kick => "KK",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Platform::Native => "native",
            Platform::Twitch => "twitch",
            Platform::YouTube => "youtube",
            Platform::Kick => "kick",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidChannel {
    #[error("enter a channel name")]
    Empty,
    #[error("\"{0}\" is not a valid {1} channel")]
    Malformed(String, &'static str),
    #[error("\"{0}\" is not of the form platform:channel")]
    UnknownPlatform(String),
}

/// A normalized channel on one platform. Written as `platform:channel`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct ChannelKey {
    platform: Platform,
    channel: String,
}

impl ChannelKey {
    /// Accepts a bare name, `#name`, `@name`, or a pasted channel URL.
    pub fn new(platform: Platform, raw: &str) -> Result<Self, InvalidChannel> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(InvalidChannel::Empty);
        }
        let name = match platform {
            Platform::YouTube => youtube_video_id(raw),
            Platform::Native | Platform::Twitch | Platform::Kick => {
                last_path_segment(raw).trim_start_matches(['#', '@'])
            }
        };
        let channel = match platform {
            Platform::YouTube => name.to_owned(),
            Platform::Native | Platform::Twitch | Platform::Kick => name.to_lowercase(),
        };
        let well_formed = !channel.is_empty()
            && channel
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !well_formed {
            return Err(InvalidChannel::Malformed(raw.to_owned(), platform.name()));
        }
        Ok(Self { platform, channel })
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    pub fn channel(&self) -> &str {
        &self.channel
    }
}

fn strip_url_scheme(raw: &str) -> Option<&str> {
    raw.strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
}

fn last_path_segment(raw: &str) -> &str {
    let Some(url) = strip_url_scheme(raw) else {
        return raw;
    };
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

fn youtube_video_id(raw: &str) -> &str {
    if let Some((_, query)) = raw.split_once('?')
        && let Some(id) = query.split('&').find_map(|pair| pair.strip_prefix("v="))
    {
        return id;
    }
    last_path_segment(raw)
}

impl fmt::Display for ChannelKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.platform.slug(), self.channel)
    }
}

impl FromStr for ChannelKey {
    type Err = InvalidChannel;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let unknown = || InvalidChannel::UnknownPlatform(s.to_owned());
        let (slug, channel) = s.split_once(':').ok_or_else(unknown)?;
        let platform = Platform::ALL
            .into_iter()
            .find(|p| p.slug() == slug)
            .ok_or_else(unknown)?;
        ChannelKey::new(platform, channel)
    }
}

impl From<ChannelKey> for String {
    fn from(key: ChannelKey) -> Self {
        key.to_string()
    }
}

impl TryFrom<String> for ChannelKey {
    type Error = InvalidChannel;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

/// Standing in a channel, lowest first so comparisons read naturally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum Role {
    Guest,
    #[default]
    Member,
    Vip,
    Moderator,
    Broadcaster,
}

impl Role {
    pub fn can_moderate(self) -> bool {
        self >= Role::Moderator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(platform: Platform, raw: &str) -> String {
        ChannelKey::new(platform, raw).unwrap().channel().to_owned()
    }

    #[test]
    fn normalizes_names_and_pasted_urls() {
        assert_eq!(channel(Platform::Native, "  #Main "), "main");
        assert_eq!(channel(Platform::Twitch, "@SomeStreamer"), "somestreamer");
        assert_eq!(
            channel(Platform::Twitch, "https://www.twitch.tv/SomeStreamer?x=1"),
            "somestreamer"
        );
        assert_eq!(
            channel(Platform::Kick, "https://kick.com/some-one/"),
            "some-one"
        );
    }

    #[test]
    fn youtube_ids_keep_case_from_every_url_shape() {
        for raw in [
            "jfKfPfyJRdk",
            "https://www.youtube.com/watch?v=jfKfPfyJRdk",
            "https://www.youtube.com/watch?app=desktop&v=jfKfPfyJRdk&t=3",
            "https://youtu.be/jfKfPfyJRdk?si=abc",
            "https://www.youtube.com/live/jfKfPfyJRdk",
        ] {
            assert_eq!(channel(Platform::YouTube, raw), "jfKfPfyJRdk", "{raw}");
        }
    }

    #[test]
    fn rejects_names_that_could_inject_protocol_text() {
        for raw in ["", "   ", "#", "a b", "chan\r\nPRIVMSG #x :hi", "a/b"] {
            assert!(ChannelKey::new(Platform::Twitch, raw).is_err(), "{raw:?}");
        }
    }

    #[test]
    fn round_trips_through_its_text_form() {
        let key: ChannelKey = "youtube:jfKfPfyJRdk".parse().unwrap();
        assert_eq!(key.platform(), Platform::YouTube);
        assert_eq!(key.to_string(), "youtube:jfKfPfyJRdk");
        assert!("discord:x".parse::<ChannelKey>().is_err());
        assert!("main".parse::<ChannelKey>().is_err());
    }

    #[test]
    fn only_moderators_and_broadcasters_moderate() {
        assert!(Role::Broadcaster.can_moderate());
        assert!(Role::Moderator.can_moderate());
        assert!(!Role::Vip.can_moderate());
        assert!(!Role::Guest.can_moderate());
    }
}
