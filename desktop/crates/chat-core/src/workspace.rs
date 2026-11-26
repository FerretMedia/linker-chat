use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::platform::ChannelKey;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaneMode {
    Single(ChannelKey),
    Merged(Vec<ChannelKey>),
}

impl PaneMode {
    pub fn channels(&self) -> &[ChannelKey] {
        match self {
            PaneMode::Single(key) => std::slice::from_ref(key),
            PaneMode::Merged(keys) => keys,
        }
    }

    pub fn is_merged(&self) -> bool {
        matches!(self, PaneMode::Merged(_))
    }

    /// Shows `key` here: a merged pane gains it, a single pane switches to it.
    pub fn show(&mut self, key: ChannelKey) {
        match self {
            PaneMode::Single(current) => *current = key,
            PaneMode::Merged(keys) if keys.contains(&key) => {}
            PaneMode::Merged(keys) => keys.push(key),
        }
    }

    pub fn title(&self) -> String {
        match self {
            PaneMode::Single(key) => key.channel().to_owned(),
            PaneMode::Merged(keys) => match keys.as_slice() {
                [] => "Empty".to_owned(),
                [only] => format!("{} (merged)", only.channel()),
                [first, rest @ ..] => format!("{} +{}", first.channel(), rest.len()),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitPane {
    pub mode: PaneMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabLayout {
    pub name: String,
    pub panes: Vec<SplitPane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    pub active_tab: usize,
    pub highlight_keywords: Vec<String>,
    /// Freeze a feed while the pointer rests on it.
    pub pause_on_hover: bool,
    pub third_party_emotes: bool,
    pub max_messages_per_channel: usize,
    pub tabs: Vec<TabLayout>,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            active_tab: 0,
            highlight_keywords: Vec::new(),
            pause_on_hover: true,
            third_party_emotes: true,
            max_messages_per_channel: 1000,
            tabs: Vec::new(),
        }
    }
}

impl WorkspaceConfig {
    /// One tab: a merged feed ready to take more platforms, beside the native room.
    pub fn starter(native: ChannelKey) -> Self {
        Self {
            tabs: vec![TabLayout {
                name: "Main".to_owned(),
                panes: vec![
                    SplitPane {
                        mode: PaneMode::Merged(vec![native.clone()]),
                    },
                    SplitPane {
                        mode: PaneMode::Single(native),
                    },
                ],
            }],
            ..Self::default()
        }
    }

    pub fn channels(&self) -> BTreeSet<ChannelKey> {
        self.tabs
            .iter()
            .flat_map(|tab| &tab.panes)
            .flat_map(|pane| pane.mode.channels())
            .cloned()
            .collect()
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> ChannelKey {
        s.parse().unwrap()
    }

    #[test]
    fn toml_round_trip_keeps_every_pane() {
        let mut config = WorkspaceConfig::starter(key("native:main"));
        config.highlight_keywords = vec!["ferret".into()];
        config.tabs[0].panes[0].mode.show(key("twitch:someone"));
        config.tabs[0].panes[0]
            .mode
            .show(key("youtube:jfKfPfyJRdk"));

        let text = config.to_toml().unwrap();
        assert_eq!(WorkspaceConfig::from_toml(&text).unwrap(), config);
        assert!(text.contains("\"twitch:someone\""), "{text}");
    }

    #[test]
    fn missing_settings_fall_back_to_defaults() {
        let config = WorkspaceConfig::from_toml("active_tab = 2").unwrap();
        assert_eq!(config.active_tab, 2);
        assert_eq!(config.max_messages_per_channel, 1000);
    }

    #[test]
    fn rejects_a_channel_that_does_not_parse() {
        let text = "[[tabs]]\nname = \"a\"\n[[tabs.panes]]\nmode = { single = \"discord:x\" }";
        assert!(WorkspaceConfig::from_toml(text).is_err());
    }

    #[test]
    fn showing_a_channel_adds_to_merged_and_replaces_single() {
        let mut merged = PaneMode::Merged(vec![key("native:main")]);
        merged.show(key("kick:a"));
        merged.show(key("kick:a"));
        assert_eq!(merged.channels(), [key("native:main"), key("kick:a")]);

        let mut single = PaneMode::Single(key("native:main"));
        single.show(key("kick:a"));
        assert_eq!(single.channels(), [key("kick:a")]);
    }
}
