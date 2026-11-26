use std::collections::BTreeSet;
use std::sync::Arc;

use chat_core::{
    ChannelKey, ChannelState, ConnectionState, LogEntry, ModAction, PaneMode, Platform, SplitPane,
    ViewState, diff_rows,
};
use gpui::{
    AnyElement, AppContext, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, ListAlignment, ListOffset, ListScrollEvent, ListState,
    MouseButton, ParentElement, Render, Stateful, StatefulInteractiveElement, Styled, Subscription,
    Window, div, list, prelude::FluentBuilder, px,
};
use providers::ProviderCommand;

use crate::assets::Icon;
use crate::chat::Chat;
use crate::row::{self, RowOptions};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme;
use crate::ui::{
    dot, icon, icon_button, icon_toggle, pill, platform_pill, primary_button, sign_in_hint, tooltip,
};

pub enum PaneEvent {
    /// The pane shows different channels now.
    LayoutChanged,
    Activated,
    SplitRequested,
    CloseRequested,
}

#[derive(Clone)]
pub struct ReplyDraft {
    pub channel: ChannelKey,
    pub message_id: String,
    pub author: String,
}

/// One split: a feed, its header, and its composer.
pub struct PaneView {
    chat: Entity<Chat>,
    mode: PaneMode,
    /// The merged channels to come back to after showing just one.
    remembered: Option<Vec<ChannelKey>>,
    /// Where a merged pane sends, if picked.
    target: Option<ChannelKey>,
    reply: Option<ReplyDraft>,

    view: ViewState,
    pause_on_hover: bool,
    hovered: bool,
    scrolled_up: bool,
    rows: Vec<Arc<LogEntry>>,
    stale: bool,
    list: ListState,

    composer: Entity<TextInput>,
    search: Entity<TextInput>,
    search_open: bool,
    active: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaneEvent> for PaneView {}

impl PaneView {
    pub fn new(
        mode: PaneMode,
        chat: Entity<Chat>,
        pause_on_hover: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| TextInput::new("", cx));
        let search = cx.new(|cx| TextInput::new("Search messages and names", cx).boxed());
        let subscriptions = vec![
            cx.observe(&chat, |pane, _, cx| pane.refresh(cx)),
            cx.subscribe(&composer, |pane, _, event, cx| match event {
                TextInputEvent::Submitted => pane.send(cx),
                TextInputEvent::Cancelled => {
                    pane.reply = None;
                    cx.notify();
                }
                TextInputEvent::Changed => {}
            }),
            cx.subscribe(&search, |pane, search, event, cx| match event {
                TextInputEvent::Changed => {
                    pane.view.search = search.read(cx).text().to_owned();
                    pane.refresh(cx);
                }
                TextInputEvent::Cancelled => pane.close_search(cx),
                TextInputEvent::Submitted => {}
            }),
        ];
        let list = ListState::new(0, ListAlignment::Bottom, px(400.));
        list.set_scroll_handler(cx.listener(|pane, event: &ListScrollEvent, _, cx| {
            pane.scrolled_up = event.is_scrolled;
            pane.update_pause(cx);
        }));
        Self {
            chat,
            mode,
            remembered: None,
            target: None,
            reply: None,
            view: ViewState::default(),
            pause_on_hover,
            hovered: false,
            scrolled_up: false,
            rows: Vec::new(),
            stale: true,
            list,
            composer,
            search,
            search_open: false,
            active: false,
            _subscriptions: subscriptions,
        }
    }

    pub fn layout(&self) -> SplitPane {
        SplitPane {
            mode: self.mode.clone(),
        }
    }

    /// Every channel this pane needs connected.
    pub fn channels(&self) -> BTreeSet<ChannelKey> {
        let remembered = self.remembered.iter().flatten();
        self.mode
            .channels()
            .iter()
            .chain(remembered)
            .cloned()
            .collect()
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        cx.notify();
    }

    /// The composer, unless the pane is read-only and shows none.
    pub fn composer_focus(&self, cx: &Context<Self>) -> Option<FocusHandle> {
        let target = self.send_target(cx)?;
        let writable = self.chat.read(cx).can_send(&target);
        writable.then(|| self.composer.focus_handle(cx))
    }

    pub fn show(&mut self, key: ChannelKey, cx: &mut Context<Self>) {
        self.mode.show(key);
        self.layout_changed(cx);
    }

    /// Switches between one channel and a merge, restoring the earlier merge if any.
    fn toggle_merged(&mut self, cx: &mut Context<Self>) {
        self.mode = match &self.mode {
            PaneMode::Merged(keys) => {
                let Some(shown) = self.send_target(cx) else {
                    return;
                };
                self.remembered = Some(keys.clone());
                PaneMode::Single(shown)
            }
            PaneMode::Single(key) => {
                let mut keys = self.remembered.take().unwrap_or_default();
                if !keys.contains(key) {
                    keys.insert(0, key.clone());
                }
                PaneMode::Merged(keys)
            }
        };
        self.layout_changed(cx);
    }

    fn remove_channel(&mut self, key: &ChannelKey, cx: &mut Context<Self>) {
        if let PaneMode::Merged(keys) = &mut self.mode {
            keys.retain(|other| other != key);
        }
        self.layout_changed(cx);
    }

    fn layout_changed(&mut self, cx: &mut Context<Self>) {
        let shown = self.mode.channels();
        self.reply.take_if(|reply| !shown.contains(&reply.channel));
        self.target.take_if(|target| !shown.contains(target));
        self.refresh(cx);
        cx.emit(PaneEvent::LayoutChanged);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.stale = true;
        cx.notify();
    }

    fn update_pause(&mut self, cx: &mut Context<Self>) {
        if self.scrolled_up || (self.hovered && self.pause_on_hover) {
            self.view.pause(self.chat.read(cx).store());
        } else {
            self.view.resume();
        }
        self.refresh(cx);
    }

    /// Jumps to the newest message. A hovering pointer pauses again only after leaving.
    fn resume(&mut self, cx: &mut Context<Self>) {
        self.hovered = false;
        self.scrolled_up = false;
        self.list.scroll_to(ListOffset {
            item_ix: self.list.item_count(),
            offset_in_item: px(0.),
        });
        self.update_pause(cx);
    }

    pub fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_open {
            // Focus falls back to the composer once the field is gone.
            self.close_search(cx);
        } else {
            self.search_open = true;
            window.focus(&self.search.focus_handle(cx));
            cx.notify();
        }
    }

    fn close_search(&mut self, cx: &mut Context<Self>) {
        self.search_open = false;
        self.search.update(cx, |search, cx| search.set_text("", cx));
        cx.notify();
    }

    pub fn start_reply(&mut self, draft: ReplyDraft, window: &mut Window, cx: &mut Context<Self>) {
        self.reply = Some(draft);
        if let Some(composer) = self.composer_focus(cx) {
            window.focus(&composer);
        }
        cx.notify();
    }

    pub fn moderate(&mut self, channel: &ChannelKey, action: ModAction, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cx| {
            chat.command(channel, ProviderCommand::Moderate(action), cx);
        });
    }

    /// Channels a message can be sent to from here.
    fn sendable(&self, cx: &Context<Self>) -> Vec<ChannelKey> {
        let chat = self.chat.read(cx);
        self.mode
            .channels()
            .iter()
            .filter(|key| chat.can_send(key))
            .cloned()
            .collect()
    }

    /// Where the composer sends: the reply's channel, the picked one, or the first that can.
    fn send_target(&self, cx: &Context<Self>) -> Option<ChannelKey> {
        let sendable = self.sendable(cx);
        self.reply
            .as_ref()
            .map(|reply| &reply.channel)
            .or(self.target.as_ref())
            .or(sendable.first())
            .or(self.mode.channels().first())
            .cloned()
    }

    fn cycle_target(&mut self, cx: &mut Context<Self>) {
        let sendable = self.sendable(cx);
        let current = self.send_target(cx);
        let next = sendable
            .iter()
            .position(|key| Some(key) == current.as_ref())
            .map_or(0, |index| (index + 1) % sendable.len());
        self.target = sendable.get(next).cloned();
        self.reply = None;
        cx.notify();
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let content = self.composer.read(cx).text().trim().to_owned();
        let Some(target) = self.send_target(cx) else {
            return;
        };
        if content.is_empty() || !self.chat.read(cx).can_send(&target) {
            return;
        }
        let reply_to = self.reply.take().map(|reply| reply.message_id);
        self.composer
            .update(cx, |composer, cx| composer.set_text("", cx));
        self.chat.update(cx, |chat, cx| {
            chat.command(&target, ProviderCommand::Send { content, reply_to }, cx);
        });
        self.resume(cx);
    }

    /// Recomputes the rows and tells the list which ones changed.
    fn sync_rows(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.stale) {
            return;
        }
        let rows = self.chat.read(cx).store().rows(&self.mode, &self.view);
        let seqs = |rows: &[Arc<LogEntry>]| rows.iter().map(|row| row.seq).collect::<Vec<_>>();
        for splice in diff_rows(&seqs(&self.rows), &seqs(&rows)) {
            self.list.splice(splice.range, splice.count);
        }
        self.rows = rows;
    }

    fn render_row(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(entry) = self.rows.get(index).cloned() else {
            return div().into_any_element();
        };
        let chat = self.chat.read(cx);
        let options = RowOptions {
            show_platform: self.mode.is_merged(),
            can_reply: chat.can_send(&entry.channel),
            can_moderate: chat
                .store()
                .channel(&entry.channel)
                .is_some_and(ChannelState::can_moderate),
        };
        row::render(&entry, options, cx)
    }

    fn connection(&self, key: &ChannelKey, cx: &Context<Self>) -> ConnectionState {
        let store = self.chat.read(cx).store();
        store
            .channel(key)
            .map_or(ConnectionState::Connecting, |state| {
                state.connection.clone()
            })
    }

    /// The state most worth showing for the whole pane.
    fn worst_connection(&self, cx: &Context<Self>) -> ConnectionState {
        let severity = |state: &ConnectionState| match state {
            ConnectionState::Connected => 0,
            ConnectionState::Connecting => 1,
            ConnectionState::Reconnecting { .. } => 2,
            ConnectionState::Disconnected => 3,
            ConnectionState::Error(_) => 4,
        };
        self.mode
            .channels()
            .iter()
            .map(|key| self.connection(key, cx))
            .max_by_key(severity)
            .unwrap_or_default()
    }

    fn viewers(&self, cx: &Context<Self>) -> Option<u64> {
        let store = self.chat.read(cx).store();
        self.mode
            .channels()
            .iter()
            .filter_map(|key| store.channel(key)?.room.viewer_count)
            .reduce(|total, count| total + count)
    }

    /// The slow mode of the channel being written to, when it is on.
    fn slow_mode(&self, cx: &Context<Self>) -> Option<u32> {
        let target = self.send_target(cx)?;
        let room = &self.chat.read(cx).store().channel(&target)?.room;
        room.slow_mode_secs.filter(|secs| *secs > 0)
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let connection = self.worst_connection(cx);
        let merged = self.mode.is_merged();
        let failed: Vec<ChannelKey> = self
            .mode
            .channels()
            .iter()
            .filter(|key| {
                matches!(
                    self.connection(key, cx),
                    ConnectionState::Error(_) | ConnectionState::Disconnected
                )
            })
            .cloned()
            .collect();
        let platforms: BTreeSet<Platform> = self
            .mode
            .channels()
            .iter()
            .map(|key| key.platform())
            .collect();

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .h(px(30.))
            .pl(px(8.))
            // Lines the last icon up with the tab bar's.
            .pr(px(4.))
            .bg(theme::color(theme::PANEL))
            .border_b_1()
            .border_color(theme::color(if self.active {
                theme::ACCENT_BRIGHT
            } else {
                theme::BORDER
            }))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap(px(4.))
                    .min_w_0()
                    .child(
                        div()
                            .id("connection")
                            .tooltip(tooltip(connection_label(&connection)))
                            .child(dot(theme::connection(&connection))),
                    )
                    .child(
                        // Clipped first when the split is narrow.
                        div()
                            .flex()
                            .gap(px(4.))
                            .min_w_0()
                            .overflow_hidden()
                            .children(platforms.into_iter().map(platform_pill)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(160.))
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(12.))
                            .child(self.mode.title()),
                    )
                    .children(self.viewers(cx).map(|count| {
                        div()
                            .id("viewers")
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .text_size(px(11.))
                            .text_color(theme::color(theme::MUTED))
                            .tooltip(tooltip("Viewers"))
                            .child(icon(Icon::Viewers, theme::MUTED))
                            .child(count.to_string())
                    }))
                    .children(
                        self.slow_mode(cx)
                            .map(|secs| pill(format!("SLOW {secs}s"), theme::WARN)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .when(self.view.is_paused(), |actions| {
                        actions.child(
                            div()
                                .id("paused")
                                .flex()
                                .items_center()
                                .h(px(22.))
                                .px(px(5.))
                                .tooltip(tooltip("Scrolling is paused"))
                                .child(icon(Icon::Pause, theme::WARN)),
                        )
                    })
                    .when(!failed.is_empty(), |actions| {
                        actions.child(icon_button("retry", Icon::Retry, "Reconnect").on_click(
                            cx.listener(move |pane, _, _, cx| {
                                pane.chat.update(cx, |chat, cx| {
                                    failed.iter().for_each(|key| chat.reconnect(key, cx));
                                });
                            }),
                        ))
                    })
                    .child(
                        icon_toggle("search", Icon::Search, "Search (Ctrl+F)", self.search_open)
                            .on_click(
                                cx.listener(|pane, _, window, cx| pane.toggle_search(window, cx)),
                            ),
                    )
                    .child(
                        icon_toggle(
                            // Its own id per state, so the tooltip follows the state.
                            if merged { "unmerge" } else { "merge" },
                            if merged { Icon::Merge } else { Icon::Single },
                            if merged {
                                "Show one channel"
                            } else {
                                "Merge channels"
                            },
                            merged,
                        )
                        .on_click(cx.listener(|pane, _, _, cx| pane.toggle_merged(cx))),
                    )
                    .child(
                        icon_button("split", Icon::Split, "Add a split").on_click(
                            cx.listener(|_, _, _, cx| cx.emit(PaneEvent::SplitRequested)),
                        ),
                    )
                    .child(
                        icon_button("close", Icon::Close, "Close this split (Ctrl+W)").on_click(
                            cx.listener(|_, _, _, cx| cx.emit(PaneEvent::CloseRequested)),
                        ),
                    ),
            )
    }

    /// The merged channels. Clicking one sends there.
    fn render_channels(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let target = self.send_target(cx);
        div()
            .id("channels")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .px(px(8.))
            .py(px(4.))
            .overflow_x_scroll()
            .bg(theme::color(theme::PANEL))
            .border_b_1()
            .border_color(theme::color(theme::BORDER))
            .children(self.mode.channels().iter().enumerate().map(|(index, key)| {
                let state = self.connection(key, cx);
                let picked = target.as_ref() == Some(key);
                let (pick, remove) = (key.clone(), key.clone());
                div()
                    .id(("channel", index))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.))
                    .h(px(22.))
                    .pl(px(6.))
                    .pr(px(2.))
                    .rounded(px(4.))
                    .bg(theme::color(theme::RAISED))
                    .border_1()
                    .text_size(px(11.))
                    .cursor_pointer()
                    .map(|chip| {
                        if picked {
                            chip.border_color(theme::color(theme::ACCENT_BRIGHT))
                        } else {
                            chip.border_color(theme::color(theme::BORDER))
                                .text_color(theme::color(theme::TEXT_SECONDARY))
                                .hover(|style| style.bg(theme::color(theme::HOVER)))
                        }
                    })
                    .tooltip(tooltip(connection_label(&state)))
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        pane.target = Some(pick.clone());
                        pane.reply = None;
                        cx.notify();
                    }))
                    .child(dot(theme::connection(&state)))
                    .child(platform_pill(key.platform()))
                    .child(
                        div()
                            .max_w(px(140.))
                            .truncate()
                            .child(key.channel().to_owned()),
                    )
                    .child(
                        icon_button(("remove", index), Icon::Close, "Remove from this merge")
                            .h(px(16.))
                            .px(px(2.))
                            .rounded(px(3.))
                            .on_click(cx.listener(move |pane, _, _, cx| {
                                cx.stop_propagation();
                                pane.remove_channel(&remove, cx);
                            })),
                    )
            }))
    }

    fn render_log(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("log")
            .flex_1()
            .min_h_0()
            .relative()
            .bg(theme::color(theme::BACKGROUND))
            .on_hover(cx.listener(|pane, hovered: &bool, _, cx| {
                pane.hovered = *hovered;
                pane.update_pause(cx);
            }))
            .when(self.rows.is_empty(), |log| log.child(self.render_empty(cx)))
            .child(
                list(
                    self.list.clone(),
                    cx.processor(|pane, index, _, cx| pane.render_row(index, cx)),
                )
                .size_full()
                .py_1(),
            )
            .children(self.render_unread(cx))
    }

    fn render_empty(&self, cx: &Context<Self>) -> Div {
        let text = if self.mode.channels().is_empty() {
            "Connect a channel to fill this feed.".to_owned()
        } else if !self.view.search.trim().is_empty() {
            format!("Nothing matches \"{}\".", self.view.search.trim())
        } else {
            match self.worst_connection(cx) {
                ConnectionState::Connected => "No messages yet.".to_owned(),
                state => connection_label(&state),
            }
        };
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .px_4()
            .text_size(px(12.5))
            .text_color(theme::color(theme::TEXT_SECONDARY))
            .child(text)
    }

    fn render_unread(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let unread = self.chat.read(cx).store().unread(&self.mode, &self.view);
        let label = match unread {
            0 if self.scrolled_up => "Jump to newest".to_owned(),
            // Nothing is being missed, so leave the last line uncovered.
            0 => return None,
            1 => "1 new message".to_owned(),
            count => format!("{count} new messages"),
        };
        let pill = div()
            .id("resume")
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(12.))
            .py(px(4.))
            .rounded_full()
            .border_1()
            .shadow_md()
            .text_size(px(11.))
            .cursor_pointer()
            .map(|pill| {
                if unread > 0 {
                    pill.bg(theme::color(theme::ACCENT))
                        .border_color(theme::color(theme::ACCENT_BRIGHT))
                        .hover(|style| style.bg(theme::color(theme::ACCENT_BRIGHT)))
                } else {
                    pill.bg(theme::color(theme::RAISED))
                        .border_color(theme::color(theme::BORDER))
                        .hover(|style| style.bg(theme::color(theme::HOVER)))
                }
            })
            .tooltip(tooltip("Jump to the newest message"))
            .on_click(cx.listener(|pane, _, _, cx| pane.resume(cx)))
            .child(icon(Icon::Single, theme::TEXT))
            .child(label);
        Some(
            div()
                .absolute()
                .bottom(px(8.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(pill),
        )
    }

    fn render_composer(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let bar = div()
            .flex()
            .flex_col()
            .flex_none()
            .bg(theme::color(theme::PANEL))
            .border_t_1()
            .border_color(theme::color(theme::BORDER));
        let Some(target) = self.send_target(cx) else {
            return bar;
        };
        if !self.chat.read(cx).can_send(&target) {
            return bar.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(px(11.5))
                    .text_color(theme::color(theme::TEXT_SECONDARY))
                    .child(sign_in_hint(&target)),
            );
        }
        let placeholder = format!("Message {}", target.channel());
        self.composer.update(cx, |composer, _| {
            composer.set_placeholder(placeholder);
        });
        let focused = self.composer.focus_handle(cx).is_focused(window);
        bar.when(focused, |bar| {
            bar.border_color(theme::color(theme::ACCENT_BRIGHT))
        })
        .children(self.reply.as_ref().map(|reply| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .h(px(26.))
                .pl(px(8.))
                .pr(px(4.))
                .border_b_1()
                .border_color(theme::color(theme::BORDER))
                .text_size(px(11.5))
                .text_color(theme::color(theme::TEXT_SECONDARY))
                .child(icon(Icon::Reply, theme::MUTED))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(format!("Replying to {}", reply.author)),
                )
                .child(
                    icon_button("cancel-reply", Icon::Close, "Cancel reply (Esc)").on_click(
                        cx.listener(|pane, _, _, cx| {
                            pane.reply = None;
                            cx.notify();
                        }),
                    ),
                )
        }))
        .child(
            div()
                .flex()
                .h(px(30.))
                .when(self.mode.is_merged(), |row| {
                    row.child(
                        platform_pill(target.platform())
                            .id("target")
                            .h_full()
                            .flex()
                            .items_center()
                            .px(px(8.))
                            .rounded_none()
                            .border_r_1()
                            .border_color(theme::color(theme::BORDER))
                            .cursor_pointer()
                            .tooltip(tooltip(format!("Sends to {target}. Click to switch.")))
                            .on_click(cx.listener(|pane, _, _, cx| pane.cycle_target(cx))),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(self.composer.clone()),
                )
                .child(
                    primary_button("send", "Send")
                        .h_full()
                        .px(px(12.))
                        .rounded_none()
                        .border_0()
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.send(cx);
                        })),
                ),
        )
    }
}

fn connection_label(state: &ConnectionState) -> String {
    match state {
        ConnectionState::Connected => "Connected".to_owned(),
        ConnectionState::Connecting => "Connecting".to_owned(),
        ConnectionState::Reconnecting { attempt } => format!("Reconnecting, attempt {attempt}"),
        ConnectionState::Disconnected => "Disconnected".to_owned(),
        ConnectionState::Error(reason) => reason.clone(),
    }
}

impl Render for PaneView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_rows(cx);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(260.))
            .min_h(px(180.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(PaneEvent::Activated)),
            )
            .child(self.render_header(cx))
            .when(self.mode.is_merged(), |pane| {
                pane.child(self.render_channels(cx))
            })
            .when(self.search_open, |pane| {
                pane.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(4.))
                        .pl(px(8.))
                        .pr(px(4.))
                        .py(px(5.))
                        .bg(theme::color(theme::PANEL))
                        .border_b_1()
                        .border_color(theme::color(theme::BORDER))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h(px(26.))
                                .child(self.search.clone()),
                        )
                        .child(
                            icon_button("close-search", Icon::Close, "Close search (Esc)")
                                .on_click(cx.listener(|pane, _, _, cx| pane.close_search(cx))),
                        ),
                )
            })
            .child(self.render_log(cx))
            .child(self.render_composer(window, cx))
    }
}
