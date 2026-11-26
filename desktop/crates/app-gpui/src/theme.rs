use std::sync::OnceLock;

use chat_core::{Color, ConnectionState, Platform};
use gpui::{Rgba, rgb, rgba};
use time::macros::format_description;
use time::{OffsetDateTime, UtcOffset};

pub const BACKGROUND: u32 = 0x09090b;
pub const PANEL: u32 = 0x111115;
pub const RAISED: u32 = 0x16161c;
pub const HOVER: u32 = 0x202028;
pub const BORDER: u32 = 0x26262c;
pub const TEXT: u32 = 0xfafafa;
pub const TEXT_SECONDARY: u32 = 0xa1a1aa;
pub const MUTED: u32 = 0x85858f;
/// Fills only. Text and icons need `ACCENT_BRIGHT`.
pub const ACCENT: u32 = 0x4545e6;
pub const ACCENT_BRIGHT: u32 = 0x6d6dff;
/// Fills only. Text and icons need `CRIMSON_BRIGHT`.
pub const CRIMSON: u32 = 0x9f1d8e;
pub const CRIMSON_BRIGHT: u32 = 0xf52bd0;
pub const REC: u32 = 0xff3131;
pub const BAD: u32 = REC;
pub const GOOD: u32 = 0x34d399;
pub const WARN: u32 = 0xf59e0b;
pub const LINK: u32 = 0x7dd3fc;

const READABLE_CONTRAST: f32 = 4.5;

pub fn color(hex: u32) -> Rgba {
    rgb(hex)
}

pub fn tint(hex: u32, alpha: u8) -> Rgba {
    rgba(hex << 8 | u32::from(alpha))
}

pub fn platform(platform: Platform) -> u32 {
    match platform {
        Platform::Native => ACCENT_BRIGHT,
        Platform::Twitch => 0xa970ff,
        Platform::Kick => 0x53fc18,
        Platform::YouTube => 0xff4e4e,
    }
}

pub fn connection(state: &ConnectionState) -> u32 {
    match state {
        ConnectionState::Connected => GOOD,
        ConnectionState::Connecting | ConnectionState::Reconnecting { .. } => WARN,
        ConnectionState::Error(_) => BAD,
        ConnectionState::Disconnected => MUTED,
    }
}

/// The author's color, lightened just enough to read on the background.
pub fn author(color: Color) -> Rgba {
    let white = rgb(0xffffff);
    let color = rgb(color.0);
    (0..=20)
        .map(|step| mix(color, white, step as f32 / 20.))
        .find(|mixed| contrast(*mixed, rgb(BACKGROUND)) >= READABLE_CONTRAST)
        .unwrap_or(white)
}

fn mix(from: Rgba, to: Rgba, amount: f32) -> Rgba {
    let channel = |from: f32, to: f32| from + (to - from) * amount;
    Rgba {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
        a: 1.,
    }
}

/// WCAG contrast ratio.
fn contrast(a: Rgba, b: Rgba) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn luminance(color: Rgba) -> f32 {
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

static LOCAL_OFFSET: OnceLock<UtcOffset> = OnceLock::new();

pub fn remember_local_offset() {
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    LOCAL_OFFSET.get_or_init(|| offset);
}

pub fn clock(timestamp: OffsetDateTime) -> String {
    let offset = LOCAL_OFFSET.get().copied().unwrap_or(UtcOffset::UTC);
    timestamp
        .to_offset(offset)
        .format(format_description!("[hour]:[minute]"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_author_color_reads_on_the_background() {
        for hex in [0x000000, 0x0000ff, 0x2222aa, 0x8b0000, 0x808080, 0xff0000] {
            let ratio = contrast(author(Color(hex)), rgb(BACKGROUND));
            assert!(ratio >= READABLE_CONTRAST, "{hex:06x} has {ratio}");
        }
    }

    #[test]
    fn every_text_color_reads_on_the_background() {
        let platforms = Platform::ALL.map(platform);
        let text = [
            TEXT,
            TEXT_SECONDARY,
            MUTED,
            ACCENT_BRIGHT,
            CRIMSON_BRIGHT,
            REC,
            GOOD,
            WARN,
            LINK,
        ];
        for hex in text.into_iter().chain(platforms) {
            let ratio = contrast(rgb(hex), rgb(BACKGROUND));
            assert!(ratio >= READABLE_CONTRAST, "{hex:06x} has {ratio}");
        }
    }

    #[test]
    fn a_color_that_already_reads_is_kept() {
        assert_eq!(author(Color(0xffd166)), rgb(0xffd166));
    }
}
