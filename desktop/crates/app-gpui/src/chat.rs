use std::collections::BTreeSet;

use chat_core::{ChannelKey, ChatEvent, ChatStore};
use gpui::Context;
use providers::{ProviderCommand, ProviderConfig, ProviderEvent, fetch_global_emotes};
use time::OffsetDateTime;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use crate::connections::Connections;

/// The store and the connections that feed it, shared by every pane.
pub struct Chat {
    store: ChatStore,
    connections: Connections,
}

impl Chat {
    pub fn new(
        store: ChatStore,
        config: ProviderConfig,
        runtime: Runtime,
        cx: &mut Context<Self>,
    ) -> Self {
        let http = config.http.clone();
        let wants_emotes = config.third_party_emotes;
        let (sender, events) = mpsc::unbounded_channel();
        let connections = Connections::new(runtime, config, sender);
        if wants_emotes {
            let emotes = connections
                .runtime()
                .spawn(async move { fetch_global_emotes(&http).await });
            cx.spawn(async move |chat, cx| {
                if let Ok(emotes) = emotes.await {
                    let _ = chat.update(cx, |chat, cx| {
                        chat.store.install_global_emotes(emotes);
                        cx.notify();
                    });
                }
            })
            .detach();
        }
        Self::pump(events, cx);
        Self { store, connections }
    }

    /// Applies events in batches, so a burst costs one redraw.
    fn pump(mut events: mpsc::UnboundedReceiver<ProviderEvent>, cx: &mut Context<Self>) {
        cx.spawn(async move |chat, cx| {
            while let Some(first) = events.recv().await {
                let mut batch = vec![first];
                while let Ok(event) = events.try_recv() {
                    batch.push(event);
                }
                let applied = chat.update(cx, |chat, cx| {
                    for ProviderEvent { channel, event } in batch {
                        // A closed channel's last events may still be in flight.
                        if chat.connections.contains(&channel) {
                            chat.apply(&channel, event);
                        }
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    pub fn store(&self) -> &ChatStore {
        &self.store
    }

    pub fn can_send(&self, key: &ChannelKey) -> bool {
        self.connections.can_send(key)
    }

    pub fn connect_exactly(&mut self, wanted: &BTreeSet<ChannelKey>, cx: &mut Context<Self>) {
        for dropped in self.connections.reconcile(wanted) {
            self.store.forget(&dropped);
        }
        cx.notify();
    }

    pub fn reconnect(&mut self, key: &ChannelKey, cx: &mut Context<Self>) {
        self.connections.reconnect(key);
        cx.notify();
    }

    pub fn command(&mut self, key: &ChannelKey, command: ProviderCommand, cx: &mut Context<Self>) {
        if !self.connections.command(key, command) {
            let text = "Disconnected, so nothing was sent.".to_owned();
            self.apply(key, ChatEvent::System(text));
            cx.notify();
        }
    }

    fn apply(&mut self, key: &ChannelKey, event: ChatEvent) {
        self.store.apply(key, event, OffsetDateTime::now_utc());
    }
}
