use std::sync::Arc;

use time::OffsetDateTime;

use crate::platform::Role;

/// 24-bit sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    const FALLBACKS: [Color; 10] = [
        Color(0xff6b6b),
        Color(0x4ecdc4),
        Color(0xffd166),
        Color(0xa78bfa),
        Color(0xf472b6),
        Color(0x38bdf8),
        Color(0x34d399),
        Color(0xfb923c),
        Color(0xfacc15),
        Color(0xc084fc),
    ];

    /// Parses `#RRGGBB`.
    pub fn parse(hex: &str) -> Option<Self> {
        let digits = hex.trim().strip_prefix('#')?;
        (digits.len() == 6)
            .then(|| u32::from_str_radix(digits, 16).ok())
            .flatten()
            .map(Color)
    }

    /// A stable color for authors whose platform gives none.
    pub fn for_name(name: &str) -> Self {
        let hash = name.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
            (hash ^ u32::from(byte.to_ascii_lowercase())).wrapping_mul(0x0100_0193)
        });
        Self::FALLBACKS[hash as usize % Self::FALLBACKS.len()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub color: Color,
    pub badges: Vec<String>,
    pub role: Role,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmoteProvider {
    Native,
    Twitch,
    Kick,
    SevenTv,
    Bttv,
    Ffz,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Emote {
    pub code: String,
    pub url: String,
    pub animated: bool,
    pub provider: EmoteProvider,
}

/// An emote the platform itself placed in the text, as a byte range of `content`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineEmote {
    pub range: std::ops::Range<usize>,
    pub emote: Emote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageFragment {
    Text(String),
    Emote(Arc<Emote>),
    /// The mentioned name, without the `@`.
    Mention(String),
    Link(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyRef {
    pub message_id: String,
    /// Empty when the platform sent only an id; the store fills it from the log.
    pub author: Option<String>,
    pub excerpt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageKind {
    Chat,
    /// `/me` text.
    Action,
    /// Super Chat or sticker, with the platform's formatted amount.
    Paid {
        amount: String,
    },
    /// Subscription, gift, or milestone, with the platform's headline.
    Membership {
        headline: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: String,
    pub author: Author,
    pub content: String,
    pub inline_emotes: Vec<InlineEmote>,
    pub reply: Option<ReplyRef>,
    pub kind: MessageKind,
    pub timestamp: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_six_digit_hex() {
        assert_eq!(Color::parse("#1E90FF"), Some(Color(0x1e90ff)));
        for bad in ["", "1E90FF", "#fff", "#1E90FG", "#1E90FF00"] {
            assert_eq!(Color::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn fallback_color_ignores_case() {
        assert_eq!(Color::for_name("Ferret"), Color::for_name("ferret"));
    }
}
