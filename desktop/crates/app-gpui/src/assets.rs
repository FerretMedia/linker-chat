use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

macro_rules! icons {
    ($($name:ident => $file:literal),* $(,)?) => {
        #[derive(Clone, Copy)]
        pub enum Icon { $($name),* }

        impl Icon {
            pub fn path(self) -> &'static str {
                match self { $(Icon::$name => $file),* }
            }
        }

        fn bytes(path: &str) -> Option<&'static [u8]> {
            match path {
                $($file => Some(include_bytes!(concat!("../assets/icons/", $file))),)*
                _ => None,
            }
        }
    };
}

icons! {
    Ban => "ban.svg",
    Clock => "clock.svg",
    Close => "close.svg",
    Linker => "linker.svg",
    Merge => "merge.svg",
    Pause => "pause.svg",
    Plug => "plug.svg",
    Plus => "plus.svg",
    Reply => "reply.svg",
    Retry => "retry.svg",
    Search => "search.svg",
    Single => "single.svg",
    Split => "split.svg",
    Trash => "trash.svg",
    Viewers => "viewers.svg",
}

/// Files compiled into the binary.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(bytes(path).map(Cow::Borrowed))
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}
