use std::collections::BTreeSet;

use chat_core::{ChannelKey, PaneMode, Platform, SplitPane, TabLayout, WorkspaceConfig};
use gpui::{
    App, AppContext, Context, Div, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Subscription, Window, actions, div, prelude::FluentBuilder, px,
};

use crate::assets::Icon;
use crate::chat::Chat;
use crate::pane::{PaneEvent, PaneView};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme;
use crate::ui::{bar_button, icon, icon_button, icon_toggle, primary_button, text_button};
use crate::{settings, ui};

const CONTEXT: &str = "Workspace";
/// Below this width splits stack instead of sitting side by side.
const STACK_BELOW: f32 = 640.;

actions!(
    workspace,
    [
        NewTab,
        NextTab,
        PreviousTab,
        ClosePane,
        ToggleSearch,
        ToggleConnect,
        Quit
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-t", NewTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-tab", NextTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-w", ClosePane, Some(CONTEXT)),
        KeyBinding::new("ctrl-f", ToggleSearch, Some(CONTEXT)),
        KeyBinding::new("ctrl-k", ToggleConnect, Some(CONTEXT)),
        KeyBinding::new("ctrl-q", Quit, Some(CONTEXT)),
    ]);
}

struct Tab {
    name: String,
    /// Never empty.
    panes: Vec<Pane>,
    active_pane: usize,
}

struct Pane {
    view: Entity<PaneView>,
    _subscription: Subscription,
}

/// Where the connect bar puts a channel.
#[derive(Clone, Copy)]
enum Destination {
    ActivePane,
    NewSplit,
}

/// The window: tabs, their splits, and the connect bar.
pub struct Workspace {
    chat: Entity<Chat>,
    config: WorkspaceConfig,
    /// Never empty.
    tabs: Vec<Tab>,
    active_tab: usize,
    connect: ConnectBar,
    focus_handle: FocusHandle,
    font: SharedString,
    _subscriptions: Vec<Subscription>,
}

struct ConnectBar {
    open: bool,
    platform: Platform,
    channel: Entity<TextInput>,
    error: Option<String>,
}

impl Workspace {
    /// `config` must have been through [`settings::opened`].
    pub fn new(
        config: WorkspaceConfig,
        chat: Entity<Chat>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let channel = cx.new(|cx| TextInput::new(channel_hint(Platform::Twitch), cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &channel,
                window,
                |workspace, _, event, window, cx| match event {
                    TextInputEvent::Submitted => {
                        workspace.connect_channel(Destination::ActivePane, window, cx);
                    }
                    TextInputEvent::Cancelled => workspace.toggle_connect(window, cx),
                    TextInputEvent::Changed => {}
                },
            ),
            // Key bindings only reach the workspace while something in it has focus.
            cx.on_focus_lost(window, |workspace, window, cx| {
                workspace.focus_active_pane(window, cx);
            }),
        ];
        let mut workspace = Self {
            chat,
            tabs: Vec::new(),
            active_tab: 0,
            connect: ConnectBar {
                open: false,
                platform: Platform::Twitch,
                channel,
                error: None,
            },
            focus_handle: cx.focus_handle(),
            font: ui::font(cx),
            _subscriptions: subscriptions,
            config,
        };
        for layout in std::mem::take(&mut workspace.config.tabs) {
            workspace.open_tab(layout, window, cx);
        }
        workspace.active_tab = workspace.config.active_tab;
        workspace.layout_changed(cx);
        workspace.focus_active_pane(window, cx);
        workspace
    }

    fn open_tab(&mut self, layout: TabLayout, window: &mut Window, cx: &mut Context<Self>) {
        let panes = layout
            .panes
            .into_iter()
            .map(|pane| self.new_pane(pane.mode, window, cx))
            .collect();
        self.tabs.push(Tab {
            name: layout.name,
            panes,
            active_pane: 0,
        });
    }

    fn new_pane(&self, mode: PaneMode, window: &mut Window, cx: &mut Context<Self>) -> Pane {
        let chat = self.chat.clone();
        let pause_on_hover = self.config.pause_on_hover;
        let view = cx.new(|cx| PaneView::new(mode, chat, pause_on_hover, cx));
        let subscription = cx.subscribe_in(&view, window, Self::pane_event);
        Pane {
            view,
            _subscription: subscription,
        }
    }

    fn pane_event(
        &mut self,
        view: &Entity<PaneView>,
        event: &PaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaneEvent::LayoutChanged => self.layout_changed(cx),
            PaneEvent::Activated => self.activate_pane(view, cx),
            PaneEvent::SplitRequested => {
                self.activate_pane(view, cx);
                let mode = view.read(cx).layout().mode;
                self.add_split(mode, window, cx);
            }
            PaneEvent::CloseRequested => self.close_pane(view, window, cx),
        }
    }

    fn tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }

    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_tab]
    }

    fn active_pane(&self) -> &Entity<PaneView> {
        let tab = self.tab();
        &tab.panes[tab.active_pane].view
    }

    fn activate_pane(&mut self, view: &Entity<PaneView>, cx: &mut Context<Self>) {
        let tab = self.tab_mut();
        if let Some(index) = tab.panes.iter().position(|pane| pane.view == *view) {
            tab.active_pane = index;
            self.mark_active(cx);
        }
    }

    fn mark_active(&self, cx: &mut Context<Self>) {
        let tab = self.tab();
        // With one split there is nothing to tell apart.
        let several = tab.panes.len() > 1;
        for (index, pane) in tab.panes.iter().enumerate() {
            let active = several && index == tab.active_pane;
            pane.view.update(cx, |view, cx| view.set_active(active, cx));
        }
        cx.notify();
    }

    fn focus_active_pane(&self, window: &mut Window, cx: &mut Context<Self>) {
        let composer = self
            .active_pane()
            .update(cx, |pane, cx| pane.composer_focus(cx));
        window.focus(composer.as_ref().unwrap_or(&self.focus_handle));
    }

    fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active_tab = index;
        self.layout_changed(cx);
        self.focus_active_pane(window, cx);
    }

    fn step_tab(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.tabs.len() as isize;
        let next = (self.active_tab as isize + step).rem_euclid(count);
        self.select_tab(next as usize, window, cx);
    }

    /// Opens a tab on the current pane's channels, so there is something to read.
    fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let layout = TabLayout {
            name: format!("Tab {}", self.tabs.len() + 1),
            panes: vec![self.active_pane().read(cx).layout()],
        };
        self.open_tab(layout, window, cx);
        self.select_tab(self.tabs.len() - 1, window, cx);
    }

    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() == 1 {
            return;
        }
        self.tabs.remove(index);
        if index < self.active_tab || self.active_tab == self.tabs.len() {
            self.active_tab -= 1;
        }
        self.select_tab(self.active_tab, window, cx);
    }

    fn add_split(&mut self, mode: PaneMode, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.new_pane(mode, window, cx);
        let tab = self.tab_mut();
        tab.active_pane += 1;
        tab.panes.insert(tab.active_pane, pane);
        self.layout_changed(cx);
        self.focus_active_pane(window, cx);
    }

    /// Closing a tab's only split closes the tab.
    fn close_pane(&mut self, view: &Entity<PaneView>, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.tab_mut();
        let Some(index) = tab.panes.iter().position(|pane| pane.view == *view) else {
            return;
        };
        if tab.panes.len() == 1 {
            self.close_tab(self.active_tab, window, cx);
            return;
        }
        tab.panes.remove(index);
        if index < tab.active_pane || tab.active_pane == tab.panes.len() {
            tab.active_pane -= 1;
        }
        self.layout_changed(cx);
        self.focus_active_pane(window, cx);
    }

    /// Connects exactly what is on screen and saves the layout.
    fn layout_changed(&mut self, cx: &mut Context<Self>) {
        let wanted: BTreeSet<ChannelKey> = self
            .tabs
            .iter()
            .flat_map(|tab| &tab.panes)
            .flat_map(|pane| pane.view.read(cx).channels())
            .collect();
        self.chat
            .update(cx, |chat, cx| chat.connect_exactly(&wanted, cx));
        self.mark_active(cx);
        settings::save(&self.layout(cx));
    }

    fn layout(&self, cx: &App) -> WorkspaceConfig {
        let panes = |tab: &Tab| -> Vec<SplitPane> {
            tab.panes
                .iter()
                .map(|pane| pane.view.read(cx).layout())
                .collect()
        };
        WorkspaceConfig {
            active_tab: self.active_tab,
            tabs: self
                .tabs
                .iter()
                .map(|tab| TabLayout {
                    name: tab.name.clone(),
                    panes: panes(tab),
                })
                .collect(),
            ..self.config.clone()
        }
    }

    fn toggle_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.connect.open = !self.connect.open;
        self.connect.error = None;
        if self.connect.open {
            window.focus(&self.connect.channel.focus_handle(cx));
        } else {
            self.focus_active_pane(window, cx);
        }
        cx.notify();
    }

    fn pick_platform(&mut self, platform: Platform, window: &mut Window, cx: &mut Context<Self>) {
        self.connect.platform = platform;
        self.connect.channel.update(cx, |channel, _| {
            channel.set_placeholder(channel_hint(platform));
        });
        window.focus(&self.connect.channel.focus_handle(cx));
        cx.notify();
    }

    fn connect_channel(
        &mut self,
        destination: Destination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.connect.channel.read(cx).text().to_owned();
        match ChannelKey::new(self.connect.platform, &text) {
            Err(error) => {
                self.connect.error = Some(error.to_string());
                cx.notify();
            }
            Ok(key) => {
                self.connect.error = None;
                self.connect
                    .channel
                    .update(cx, |channel, cx| channel.set_text("", cx));
                match destination {
                    Destination::ActivePane => {
                        let pane = self.active_pane().clone();
                        pane.update(cx, |pane, cx| pane.show(key, cx));
                    }
                    Destination::NewSplit => {
                        self.add_split(PaneMode::Single(key), window, cx);
                    }
                }
            }
        }
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let closable = self.tabs.len() > 1;
        div()
            .flex()
            .flex_none()
            .h(px(30.))
            .bg(theme::color(theme::PANEL))
            .border_b_1()
            .border_color(theme::color(theme::BORDER))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .h_full()
                    .px(px(9.))
                    .border_r_1()
                    .border_color(theme::color(theme::BORDER))
                    .child(icon(Icon::Linker, theme::ACCENT_BRIGHT)),
            )
            .child(
                div()
                    .id("tabs")
                    .flex()
                    .gap_0()
                    .h_full()
                    .min_w_0()
                    .overflow_x_scroll()
                    .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                        // GPUI gives an element one border color, so the divider and the accent
                        // need an element each.
                        div()
                            .flex_none()
                            .h_full()
                            .border_r_1()
                            .border_color(theme::color(theme::BORDER))
                            .child(self.render_tab(index, tab, closable, cx))
                    })),
            )
            .child(
                bar_button(icon_button("new-tab", Icon::Plus, "New tab (Ctrl+T)"))
                    .border_r_1()
                    .on_click(
                        cx.listener(|workspace, _, window, cx| workspace.new_tab(window, cx)),
                    ),
            )
            .child(div().flex_1())
            .child(
                bar_button(icon_button("add-split", Icon::Split, "Add a split"))
                    .border_l_1()
                    .on_click(cx.listener(|workspace, _, window, cx| {
                        let mode = workspace.active_pane().read(cx).layout().mode;
                        workspace.add_split(mode, window, cx);
                    })),
            )
            .child(
                bar_button(icon_toggle(
                    "connect",
                    Icon::Plug,
                    "Connect a channel (Ctrl+K)",
                    self.connect.open,
                ))
                .border_l_1()
                .on_click(cx.listener(|workspace, _, window, cx| {
                    workspace.toggle_connect(window, cx);
                })),
            )
    }

    fn render_tab(
        &self,
        index: usize,
        tab: &Tab,
        closable: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let active = index == self.active_tab;
        div()
            .id(("tab", index))
            .flex()
            .items_center()
            .gap_1()
            .h_full()
            .pl(px(10.))
            .pr(px(if closable { 2. } else { 10. }))
            .border_t_2()
            .text_size(px(12.))
            .cursor_pointer()
            .map(|cell| {
                if active {
                    cell.bg(theme::color(theme::BACKGROUND))
                        .border_color(theme::color(theme::ACCENT_BRIGHT))
                        .text_color(theme::color(theme::TEXT))
                } else {
                    cell.bg(theme::color(theme::PANEL))
                        .border_color(gpui::transparent_black())
                        .text_color(theme::color(theme::TEXT_SECONDARY))
                        .hover(|style| style.bg(theme::color(theme::RAISED)))
                }
            })
            .on_click(cx.listener(move |workspace, _, window, cx| {
                workspace.select_tab(index, window, cx);
            }))
            .child(tab.name.clone())
            .when(closable, |cell| {
                cell.child(
                    icon_button(("close-tab", index), Icon::Close, "Close tab")
                        .h_full()
                        .px(px(5.))
                        .on_click(cx.listener(move |workspace, _, window, cx| {
                            cx.stop_propagation();
                            workspace.close_tab(index, window, cx);
                        })),
                )
            })
    }

    fn render_connect(&self, narrow: bool, window: &Window, cx: &mut Context<Self>) -> Div {
        let focused = self.connect.channel.focus_handle(cx).is_focused(window);
        let platforms =
            div()
                .flex()
                .flex_none()
                .h(px(30.))
                .children(Platform::ALL.into_iter().map(|platform| {
                    div()
                        .flex()
                        .flex_none()
                        .when(narrow, |cell| cell.flex_1())
                        .h_full()
                        .border_r_1()
                        .border_color(theme::color(theme::BORDER))
                        .child(self.render_platform(platform, cx))
                }));
        let channel = div()
            .flex()
            .h(px(30.))
            .min_w_0()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.connect.channel.clone()),
            )
            .child(
                bar_button(primary_button("connect-here", "Add to this split"))
                    .border_0()
                    .border_l_1()
                    .px(px(10.))
                    .border_color(theme::color(theme::BORDER))
                    .on_click(cx.listener(|workspace, _, window, cx| {
                        workspace.connect_channel(Destination::ActivePane, window, cx);
                    })),
            )
            .child(
                bar_button(text_button("connect-split", "Open in new split"))
                    .border_0()
                    .border_l_1()
                    .px(px(10.))
                    .on_click(cx.listener(|workspace, _, window, cx| {
                        workspace.connect_channel(Destination::NewSplit, window, cx);
                    })),
            );
        div()
            .flex()
            .flex_none()
            .flex_col()
            .bg(theme::color(theme::PANEL))
            .border_b_1()
            .border_color(theme::color(if focused {
                theme::ACCENT_BRIGHT
            } else {
                theme::BORDER
            }))
            .map(|bar| {
                if narrow {
                    bar.child(platforms).child(
                        channel
                            .border_t_1()
                            .border_color(theme::color(theme::BORDER)),
                    )
                } else {
                    bar.child(div().flex().child(platforms).child(channel.flex_1()))
                }
            })
            .children(self.connect.error.clone().map(|error| {
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .border_t_1()
                    .border_color(theme::color(theme::BORDER))
                    .bg(theme::tint(theme::BAD, 0x18))
                    .text_size(px(11.5))
                    .text_color(theme::color(theme::BAD))
                    .child(error)
            }))
    }

    /// Picked like a tab: an accent line on top, in the platform's color.
    fn render_platform(&self, platform: Platform, cx: &mut Context<Self>) -> Stateful<Div> {
        let picked = platform == self.connect.platform;
        let hex = theme::platform(platform);
        div()
            .id(platform.name())
            .flex()
            .flex_grow()
            .items_center()
            .justify_center()
            .h_full()
            .px(px(10.))
            .border_t_2()
            .text_size(px(11.5))
            .cursor_pointer()
            .map(|cell| {
                if picked {
                    cell.bg(theme::color(theme::BACKGROUND))
                        .border_color(theme::color(hex))
                        .text_color(theme::color(hex))
                } else {
                    cell.border_color(gpui::transparent_black())
                        .text_color(theme::color(theme::TEXT_SECONDARY))
                        .hover(|style| style.bg(theme::color(theme::RAISED)))
                }
            })
            .on_click(cx.listener(move |workspace, _, window, cx| {
                workspace.pick_platform(platform, window, cx);
            }))
            .child(platform.name())
    }
}

fn channel_hint(platform: Platform) -> &'static str {
    match platform {
        Platform::Native => "Room name",
        Platform::Twitch => "Twitch channel",
        Platform::Kick => "Kick channel or chatroom id",
        Platform::YouTube => "YouTube video link or id",
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stacked = window.viewport_size().width < px(STACK_BELOW);
        let panes = self.tab().panes.iter().map(|pane| pane.view.clone());
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|workspace, _: &NewTab, window, cx| {
                workspace.new_tab(window, cx);
            }))
            .on_action(cx.listener(|workspace, _: &NextTab, window, cx| {
                workspace.step_tab(1, window, cx);
            }))
            .on_action(cx.listener(|workspace, _: &PreviousTab, window, cx| {
                workspace.step_tab(-1, window, cx);
            }))
            .on_action(cx.listener(|workspace, _: &ClosePane, window, cx| {
                let pane = workspace.active_pane().clone();
                workspace.close_pane(&pane, window, cx);
            }))
            .on_action(cx.listener(|workspace, _: &ToggleSearch, window, cx| {
                let pane = workspace.active_pane().clone();
                pane.update(cx, |pane, cx| pane.toggle_search(window, cx));
            }))
            .on_action(cx.listener(|workspace, _: &ToggleConnect, window, cx| {
                workspace.toggle_connect(window, cx);
            }))
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .size_full()
            .flex()
            .flex_col()
            .font_family(self.font.clone())
            .bg(theme::color(theme::BACKGROUND))
            .text_color(theme::color(theme::TEXT))
            .child(self.render_tabs(cx))
            .when(self.connect.open, |workspace| {
                workspace.child(self.render_connect(stacked, window, cx))
            })
            .child(
                div()
                    .id("panes")
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap(px(1.))
                    .bg(theme::color(theme::BORDER))
                    .map(|area| {
                        if stacked {
                            area.flex_col().overflow_y_scroll()
                        } else {
                            area.flex_row().overflow_x_scroll()
                        }
                    })
                    .children(panes),
            )
    }
}
