//! Small pieces every view shares.

use chat_core::{ChannelKey, Platform};
use gpui::{
    AnyView, App, AppContext, Context, Div, ElementId, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement, Styled, Svg, Window,
    div, px, svg,
};

use crate::assets::Icon;
use crate::theme;

pub fn icon(icon: Icon, hex: u32) -> Svg {
    svg()
        .path(icon.path())
        .size(px(13.))
        .flex_none()
        .text_color(theme::color(hex))
}

pub fn icon_button(id: impl Into<ElementId>, glyph: Icon, label: &'static str) -> Stateful<Div> {
    button_base(id, label)
        .px(px(5.))
        .child(icon(glyph, theme::MUTED))
}

/// `icon_button` in the on position.
pub fn icon_toggle(
    id: impl Into<ElementId>,
    glyph: Icon,
    label: &'static str,
    on: bool,
) -> Stateful<Div> {
    if on {
        button_base(id, label)
            .px(px(5.))
            .bg(theme::tint(theme::ACCENT, 0x33))
            .child(icon(glyph, theme::ACCENT_BRIGHT))
    } else {
        icon_button(id, glyph, label)
    }
}

fn button_base(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(22.))
        .cursor_pointer()
        .hover(|style| style.bg(theme::color(theme::HOVER)))
        .tooltip(tooltip(label))
}

/// A button that fills the height of the bar it sits in. The caller picks the side of its hairline.
pub fn bar_button(button: Stateful<Div>) -> Stateful<Div> {
    button
        .flex_none()
        .h_full()
        .px(px(8.))
        .border_color(theme::color(theme::BORDER))
}

pub fn text_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    text_button_base(id, label)
        .bg(theme::color(theme::RAISED))
        .border_color(theme::color(theme::BORDER))
        .hover(|style| style.bg(theme::color(theme::HOVER)))
}

/// The one action a bar is for.
pub fn primary_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    text_button_base(id, label)
        .bg(theme::color(theme::ACCENT))
        .border_color(theme::color(theme::ACCENT))
        .text_color(theme::color(theme::TEXT))
        .hover(|style| style.bg(theme::color(theme::ACCENT_BRIGHT)))
}

fn text_button_base(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(24.))
        .px(px(8.))
        .border_1()
        .cursor_pointer()
        .text_size(px(11.5))
        .child(label.into())
}

pub fn platform_pill(platform: Platform) -> Div {
    let hex = theme::platform(platform);
    pill(platform.tag(), hex)
}

pub fn pill(label: impl Into<SharedString>, hex: u32) -> Div {
    div()
        .flex_none()
        .px(px(3.5))
        .text_size(px(9.5))
        .line_height(px(13.))
        .font_weight(gpui::FontWeight::BOLD)
        .bg(theme::tint(hex, 0x22))
        .text_color(theme::color(hex))
        .child(label.into())
}

pub fn dot(hex: u32) -> Div {
    div()
        .flex_none()
        .size(px(6.))
        .rounded_full()
        .bg(theme::color(hex))
}

/// What to do about a channel that cannot be written to.
pub fn sign_in_hint(key: &ChannelKey) -> &'static str {
    match key.platform() {
        Platform::Native => "Read-only. Set LINKER_CHAT_TOKEN or LINKER_CHAT_NICKNAME to chat.",
        Platform::Twitch => {
            "Read-only. Set LINKER_CHAT_TWITCH_LOGIN and LINKER_CHAT_TWITCH_TOKEN to chat."
        }
        Platform::Kick => "Read-only. Set LINKER_CHAT_KICK_TOKEN to chat.",
        Platform::YouTube => "Read-only. Set LINKER_CHAT_YOUTUBE_TOKEN to chat.",
    }
}

pub fn tooltip(label: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let label = label.into();
    move |_, cx| cx.new(|_| Tooltip(label.clone())).into()
}

struct Tooltip(SharedString);

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .bg(theme::color(theme::RAISED))
            .border_1()
            .border_color(theme::color(theme::BORDER))
            .text_size(px(11.5))
            .text_color(theme::color(theme::TEXT))
            .child(self.0.clone())
    }
}

/// The first of these families that is installed. GPUI's own default is often missing on Linux.
pub fn font(cx: &App) -> SharedString {
    let installed = cx.text_system().all_font_names();
    [
        "Inter",
        "Adwaita Sans",
        "Noto Sans",
        "Cantarell",
        "Ubuntu",
        "DejaVu Sans",
        "Liberation Sans",
    ]
    .into_iter()
    .find(|family| installed.iter().any(|name| name == family))
    .unwrap_or(".SystemUIFont")
    .into()
}
