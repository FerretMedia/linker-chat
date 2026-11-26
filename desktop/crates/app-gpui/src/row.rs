use std::sync::Arc;

use chat_core::{
    EntryBody, Highlight, LogEntry, LoggedMessage, MessageFragment, MessageKind, ModAction,
    Platform, Removal, Role, UserRef,
};
use gpui::{
    AnyElement, Context, Div, FontWeight, InteractiveElement, IntoElement, ParentElement, Rgba,
    SharedString, Stateful, StatefulInteractiveElement, Styled, StyledImage, div, img,
    prelude::FluentBuilder, px,
};

use crate::assets::Icon;
use crate::pane::{PaneView, ReplyDraft};
use crate::theme;
use crate::ui::{icon, icon_button, pill, platform_pill, tooltip};

const QUICK_TIMEOUT_SECS: u64 = 600;
const EMOTE_HEIGHT: f32 = 18.;
const LINE_HEIGHT: f32 = 18.;

pub struct RowOptions {
    pub show_platform: bool,
    pub can_reply: bool,
    pub can_moderate: bool,
}

pub fn render(
    entry: &Arc<LogEntry>,
    options: RowOptions,
    cx: &mut Context<PaneView>,
) -> AnyElement {
    let line = div()
        .id(("row", entry.seq))
        .group("row")
        .relative()
        .w_full()
        .px(px(6.))
        .py(px(1.))
        .border_l_2()
        .border_color(gpui::transparent_black())
        .line_height(px(LINE_HEIGHT))
        .text_size(px(12.5));
    match &entry.body {
        EntryBody::Notice(text) => line
            .child(
                wrapping_line()
                    .child(timestamp(entry))
                    .when(options.show_platform, |line| line.child(origin(entry)))
                    .text_color(theme::color(theme::MUTED))
                    .children(words(text, false)),
            )
            .into_any_element(),
        EntryBody::Message(logged) => message_row(line, entry, logged, options, cx),
    }
}

fn message_row(
    line: Stateful<Div>,
    entry: &Arc<LogEntry>,
    logged: &LoggedMessage,
    options: RowOptions,
    cx: &mut Context<PaneView>,
) -> AnyElement {
    let message = &logged.message;
    let removed = logged.removal.is_some();
    let body = wrapping_line()
        .child(timestamp(entry))
        .when(options.show_platform, |line| line.child(origin(entry)))
        .children(badges(logged))
        .child(author(logged))
        .when(message.kind == MessageKind::Action, |body| {
            body.italic()
                .text_color(theme::author(message.author.color))
        })
        .children(
            logged
                .fragments
                .iter()
                .enumerate()
                .flat_map(|(index, fragment)| fragment_elements(entry, index, fragment, removed)),
        )
        .children(removal_label(logged.removal));

    let highlight = highlight_colors(logged);
    let reply = options.can_reply.then(|| ReplyDraft {
        channel: entry.channel.clone(),
        message_id: message.id.clone(),
        author: message.author.display_name.clone(),
    });
    line.when_some(highlight, |line, (edge, fill)| {
        line.border_color(theme::color(edge)).bg(fill)
    })
    .hover(|style| style.bg(theme::color(theme::RAISED)))
    .children(headline(message))
    .children(reply_context(message))
    .child(body.when(removed, |body| body.opacity(0.5)))
    .when_some(reply.clone(), |line, draft| {
        line.cursor_pointer()
            .on_click(cx.listener(move |pane, _, window, cx| {
                pane.start_reply(draft.clone(), window, cx);
            }))
    })
    .child(quick_actions(entry, logged, reply, &options, cx))
    .into_any_element()
}

fn wrapping_line() -> Div {
    div().flex().flex_wrap().items_center().w_full().min_w_0()
}

fn timestamp(entry: &LogEntry) -> Div {
    div()
        .flex_none()
        .mr(px(4.))
        .text_size(px(10.5))
        .text_color(theme::color(theme::MUTED))
        .child(theme::clock(entry.timestamp))
}

fn origin(entry: &LogEntry) -> Stateful<Div> {
    platform_pill(entry.channel.platform())
        .id(("origin", entry.seq))
        .mr(px(4.))
        .tooltip(tooltip(entry.channel.to_string()))
}

fn badges(logged: &LoggedMessage) -> impl Iterator<Item = Div> {
    let author = &logged.message.author;
    let role = match author.role {
        Role::Broadcaster => Some(("HOST", theme::REC)),
        Role::Moderator => Some(("MOD", theme::GOOD)),
        Role::Vip => Some(("VIP", theme::CRIMSON_BRIGHT)),
        Role::Guest => Some(("GUEST", theme::MUTED)),
        Role::Member => None,
    };
    let subscriber = author
        .badges
        .iter()
        .any(|badge| matches!(badge.as_str(), "subscriber" | "founder" | "member"))
        .then_some(("SUB", theme::ACCENT_BRIGHT));
    role.into_iter()
        .chain(subscriber)
        .map(|(label, hex)| pill(label, hex).mr(px(4.)))
}

fn author(logged: &LoggedMessage) -> Div {
    let author = &logged.message.author;
    let separator = match logged.message.kind {
        MessageKind::Action => "",
        _ => ":",
    };
    div()
        .flex_none()
        .mr(px(5.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::author(author.color))
        .child(format!("{}{separator}", author.display_name))
}

/// One element per word, so a line can wrap between any two.
fn words(text: &str, struck: bool) -> Vec<AnyElement> {
    text.split_inclusive(char::is_whitespace)
        .map(|word| {
            let spaced = word.ends_with(char::is_whitespace);
            let word = word.trim_end();
            div()
                .max_w_full()
                .when(spaced, |word| word.mr(px(3.5)))
                .when(struck, |word| word.line_through())
                .child(SharedString::from(word.to_owned()))
                .into_any_element()
        })
        .collect()
}

fn fragment_elements(
    entry: &LogEntry,
    index: usize,
    fragment: &MessageFragment,
    struck: bool,
) -> Vec<AnyElement> {
    let id = |name: &'static str| (SharedString::from(format!("{name}-{}", entry.seq)), index);
    match fragment {
        MessageFragment::Text(text) => words(text, struck),
        MessageFragment::Mention(name) => vec![
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::color(theme::ACCENT_BRIGHT))
                .child(format!("@{name}"))
                .into_any_element(),
        ],
        MessageFragment::Link(url) => {
            let target = url.clone();
            vec![
                div()
                    .id(id("link"))
                    .max_w_full()
                    .cursor_pointer()
                    .underline()
                    .text_color(theme::color(theme::LINK))
                    .child(url.clone())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.open_url(&target);
                    })
                    .into_any_element(),
            ]
        }
        MessageFragment::Emote(emote) => {
            let code: SharedString = emote.code.clone().into();
            let fallback = code.clone();
            vec![
                div()
                    .id(id("emote"))
                    .flex_none()
                    .tooltip(tooltip(code))
                    .child(
                        img(emote.url.clone())
                            .id(id("image"))
                            .h(px(EMOTE_HEIGHT))
                            .with_fallback(move || {
                                div().child(fallback.clone()).into_any_element()
                            }),
                    )
                    .into_any_element(),
            ]
        }
    }
}

/// The edge and the fill of a row that stands out.
fn highlight_colors(logged: &LoggedMessage) -> Option<(u32, Rgba)> {
    let wash = |hex| Some((hex, theme::tint(hex, 0x1c)));
    match (&logged.highlight, &logged.message.kind) {
        (_, MessageKind::Paid { .. }) => wash(theme::WARN),
        (_, MessageKind::Membership { .. }) => wash(theme::GOOD),
        (Some(Highlight::Mention), _) => {
            Some((theme::CRIMSON_BRIGHT, theme::tint(theme::CRIMSON, 0x33)))
        }
        (Some(Highlight::Keyword(_)), _) => wash(theme::LINK),
        (Some(Highlight::Event) | None, _) => None,
    }
}

fn headline(message: &chat_core::ChatMessage) -> Option<Div> {
    let (text, hex) = match &message.kind {
        MessageKind::Paid { amount } => (amount, theme::WARN),
        MessageKind::Membership { headline } => (headline, theme::GOOD),
        MessageKind::Chat | MessageKind::Action => return None,
    };
    Some(
        div()
            .text_size(px(11.5))
            .font_weight(FontWeight::BOLD)
            .text_color(theme::color(hex))
            .child(text.clone()),
    )
}

fn reply_context(message: &chat_core::ChatMessage) -> Option<Div> {
    let reply = message.reply.as_ref()?;
    let author = reply.author.as_deref().unwrap_or("an earlier message");
    let text = match &reply.excerpt {
        Some(excerpt) => format!("{author}: {excerpt}"),
        None => author.to_owned(),
    };
    Some(
        div()
            .flex()
            .items_center()
            .gap_1()
            .line_height(px(14.))
            .text_size(px(10.5))
            .text_color(theme::color(theme::TEXT_SECONDARY))
            .child(icon(Icon::Reply, theme::MUTED).size(px(10.)))
            .child(div().truncate().child(text)),
    )
}

fn removal_label(removal: Option<Removal>) -> Option<Div> {
    let label = match removal? {
        Removal::Deleted => "deleted".to_owned(),
        Removal::TimedOut { secs } => format!("timed out {secs}s"),
        Removal::Banned => "banned".to_owned(),
    };
    Some(pill(label, theme::BAD).ml(px(4.)))
}

fn quick_button(id: &'static str, seq: u64, glyph: Icon, label: &'static str) -> Stateful<Div> {
    icon_button((id, seq), glyph, label)
        .h(px(18.))
        .px(px(4.))
        .rounded(px(3.))
}

/// Buttons that appear over the row under the pointer.
fn quick_actions(
    entry: &Arc<LogEntry>,
    logged: &LoggedMessage,
    reply: Option<ReplyDraft>,
    options: &RowOptions,
    cx: &mut Context<PaneView>,
) -> Div {
    let seq = entry.seq;
    let moderates = options.can_moderate
        && entry.channel.platform() == Platform::Native
        && logged.removal.is_none();
    let target = UserRef::of(&logged.message.author);
    let moderate = |id: &'static str, glyph, label, action: ModAction| {
        let channel = entry.channel.clone();
        quick_button(id, seq, glyph, label).on_click(cx.listener(move |pane, _, _, cx| {
            cx.stop_propagation();
            pane.moderate(&channel, action.clone(), cx);
        }))
    };
    // As tall as a one-line row, so the row below cannot paint over it.
    div()
        .absolute()
        .top(px(0.))
        .right(px(6.))
        .h(px(LINE_HEIGHT + 2.))
        .flex()
        .items_center()
        .rounded(px(5.))
        .bg(theme::color(theme::RAISED))
        .border_1()
        .border_color(theme::color(theme::BORDER))
        .invisible()
        .when(reply.is_some() || moderates, |bar| {
            bar.group_hover("row", |style| style.visible())
        })
        .when_some(reply, |bar, draft| {
            bar.child(
                quick_button("reply", seq, Icon::Reply, "Reply").on_click(cx.listener(
                    move |pane, _, window, cx| {
                        cx.stop_propagation();
                        pane.start_reply(draft.clone(), window, cx);
                    },
                )),
            )
        })
        .when(moderates, |bar| {
            bar.child(moderate(
                "delete",
                Icon::Trash,
                "Delete message",
                ModAction::Delete {
                    message_id: logged.message.id.clone(),
                },
            ))
            .child(moderate(
                "timeout",
                Icon::Clock,
                "Time out for 10 minutes",
                ModAction::Timeout {
                    target: target.clone(),
                    secs: QUICK_TIMEOUT_SECS,
                    reason: None,
                },
            ))
            .child(moderate(
                "ban",
                Icon::Ban,
                "Ban",
                ModAction::Ban {
                    target,
                    reason: None,
                },
            ))
        })
}
