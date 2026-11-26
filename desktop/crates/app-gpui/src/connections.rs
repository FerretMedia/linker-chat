use std::collections::{BTreeSet, HashMap};

use chat_core::ChannelKey;
use providers::{
    EventSender, ProviderCommand, ProviderConfig, ProviderHandle, ReconnectPolicy, connect, spawn,
};
use tokio::runtime::Runtime;

/// One live provider per channel that some pane wants.
pub struct Connections {
    runtime: Runtime,
    config: ProviderConfig,
    events: EventSender,
    handles: HashMap<ChannelKey, ProviderHandle>,
}

impl Connections {
    pub fn new(runtime: Runtime, config: ProviderConfig, events: EventSender) -> Self {
        Self {
            runtime,
            config,
            events,
            handles: HashMap::new(),
        }
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    /// Connects what is newly wanted and drops the rest. Returns what it dropped.
    pub fn reconcile(&mut self, wanted: &BTreeSet<ChannelKey>) -> Vec<ChannelKey> {
        let unwanted: Vec<ChannelKey> = self
            .handles
            .keys()
            .filter(|key| !wanted.contains(key))
            .cloned()
            .collect();
        for key in &unwanted {
            self.handles.remove(key);
        }
        for key in wanted {
            if !self.handles.contains_key(key) {
                self.reconnect(key);
            }
        }
        unwanted
    }

    /// Replaces the channel's connection with a fresh one.
    pub fn reconnect(&mut self, key: &ChannelKey) {
        self.handles.remove(key);
        let _runtime = self.runtime.enter();
        let handle = spawn(
            connect(key.clone(), &self.config),
            self.events.clone(),
            ReconnectPolicy::default(),
        );
        self.handles.insert(key.clone(), handle);
    }

    pub fn contains(&self, key: &ChannelKey) -> bool {
        self.handles.contains_key(key)
    }

    pub fn can_send(&self, key: &ChannelKey) -> bool {
        self.handles.get(key).is_some_and(ProviderHandle::can_send)
    }

    /// `false` if the channel's provider has stopped.
    pub fn command(&self, key: &ChannelKey, command: ProviderCommand) -> bool {
        self.handles
            .get(key)
            .is_some_and(|handle| handle.command(command))
    }
}
