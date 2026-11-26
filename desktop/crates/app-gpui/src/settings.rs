use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{fs, io};

use chat_core::{ChannelKey, PaneMode, Platform, SplitPane, WorkspaceConfig};

pub struct NativeRoom {
    pub key: ChannelKey,
    /// Whether the environment named it, rather than falling back to `main`.
    pub requested: bool,
}

pub fn native_room(env: impl Fn(&str) -> Option<String>) -> NativeRoom {
    let requested = env("LINKER_CHAT_ROOM").and_then(|room| {
        ChannelKey::new(Platform::Native, &room)
            .inspect_err(|error| eprintln!("Ignoring LINKER_CHAT_ROOM: {error}"))
            .ok()
    });
    NativeRoom {
        requested: requested.is_some(),
        key: requested.unwrap_or_else(|| {
            ChannelKey::new(Platform::Native, "main").expect("a valid room name")
        }),
    }
}

/// The saved layout made safe to open: at least one tab, none of them empty, and
/// the room the environment asked for on screen.
pub fn opened(mut saved: WorkspaceConfig, room: NativeRoom) -> WorkspaceConfig {
    saved.tabs.retain(|tab| !tab.panes.is_empty());
    if saved.tabs.is_empty() {
        saved.tabs = WorkspaceConfig::starter(room.key.clone()).tabs;
    }
    saved.active_tab = saved.active_tab.min(saved.tabs.len() - 1);
    if room.requested && !saved.channels().contains(&room.key) {
        saved.tabs[saved.active_tab].panes.push(SplitPane {
            mode: PaneMode::Single(room.key),
        });
    }
    saved
}

pub fn guest_nickname() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    format!("desktop-{:04}", nanos / 1000 % 10_000)
}

fn path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("LINKER_CHAT_CONFIG") {
        return Some(path.into());
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| Some(Path::new(&std::env::var_os("HOME")?).join(".config")))?;
    Some(base.join("linker-chat/workspace.toml"))
}

pub fn load() -> WorkspaceConfig {
    path().map_or_else(WorkspaceConfig::default, |path| load_from(&path))
}

fn load_from(path: &Path) -> WorkspaceConfig {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!("Could not read {}: {error}", path.display());
            }
            return WorkspaceConfig::default();
        }
    };
    WorkspaceConfig::from_toml(&text).unwrap_or_else(|error| {
        // Saving would overwrite the file, so set it aside first.
        let aside = path.with_extension("toml.invalid");
        eprintln!(
            "Could not parse {}, moving it to {}: {error}",
            path.display(),
            aside.display()
        );
        let _ = fs::rename(path, aside);
        WorkspaceConfig::default()
    })
}

pub fn save(config: &WorkspaceConfig) {
    let Some(path) = path() else { return };
    if let Err(error) = save_to(&path, config) {
        eprintln!("Could not save {}: {error}", path.display());
    }
}

fn save_to(path: &Path, config: &WorkspaceConfig) -> io::Result<()> {
    let text = config.to_toml().map_err(io::Error::other)?;
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    // Write beside the file and swap it in, so a crash cannot leave half of it.
    let draft = path.with_extension("toml.new");
    fs::write(&draft, text)?;
    fs::rename(draft, path)
}

#[cfg(test)]
mod tests {
    use chat_core::TabLayout;

    use super::*;

    fn folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("linker-chat-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        folder
    }

    #[test]
    fn saves_into_a_new_folder_and_loads_back() {
        let folder = folder("save");
        let path = folder.join("nested/workspace.toml");
        let config = WorkspaceConfig::starter("native:main".parse().unwrap());
        save_to(&path, &config).unwrap();
        assert_eq!(load_from(&path), config);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_broken_file_is_set_aside_not_overwritten() {
        let folder = folder("broken");
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("workspace.toml");
        fs::write(&path, "tabs = 3").unwrap();
        assert_eq!(load_from(&path), WorkspaceConfig::default());
        let kept = fs::read_to_string(folder.join("workspace.toml.invalid")).unwrap();
        assert_eq!(kept, "tabs = 3");
        fs::remove_dir_all(folder).unwrap();
    }

    fn room(name: &str, requested: bool) -> NativeRoom {
        NativeRoom {
            key: format!("native:{name}").parse().unwrap(),
            requested,
        }
    }

    #[test]
    fn opening_nothing_gives_the_starter_layout_and_keeps_other_settings() {
        let saved = WorkspaceConfig {
            active_tab: 7,
            highlight_keywords: vec!["ferret".into()],
            ..WorkspaceConfig::default()
        };
        let opened = opened(saved, room("main", false));
        assert_eq!(opened.active_tab, 0);
        assert_eq!(opened.tabs[0].panes.len(), 2);
        assert_eq!(opened.highlight_keywords, ["ferret"]);
    }

    #[test]
    fn opening_a_saved_layout_keeps_its_tabs_whatever_the_active_index() {
        let mut saved = WorkspaceConfig::starter("native:main".parse().unwrap());
        saved.active_tab = 9;
        let tabs = saved.tabs.clone();
        saved.tabs.push(TabLayout {
            name: "Empty".into(),
            panes: vec![],
        });
        let opened = opened(saved, room("other", false));
        assert_eq!(opened.tabs, tabs);
        assert_eq!(opened.active_tab, 0);
    }

    #[test]
    fn a_requested_room_is_added_once() {
        let saved = WorkspaceConfig::starter("native:main".parse().unwrap());
        let with_lounge = opened(saved, room("lounge", true));
        assert_eq!(with_lounge.tabs[0].panes.len(), 3);
        let again = opened(with_lounge, room("lounge", true));
        assert_eq!(again.tabs[0].panes.len(), 3);
    }

    #[test]
    fn the_room_comes_from_the_environment_or_is_main() {
        let named = native_room(|_| Some("#Lounge".into()));
        assert_eq!(named.key.to_string(), "native:lounge");
        assert!(named.requested);
        for fallback in [None, Some("not a room".to_owned())] {
            let room = native_room(|_| fallback.clone());
            assert_eq!(room.key.to_string(), "native:main");
            assert!(!room.requested);
        }
    }
}
