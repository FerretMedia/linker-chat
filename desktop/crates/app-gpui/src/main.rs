mod assets;
mod chat;
mod connections;
mod http;
mod line_editor;
mod pane;
mod row;
mod settings;
mod text_input;
mod theme;
mod ui;
mod workspace;

use std::sync::Arc;

use chat_core::ChatStore;
use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};
use providers::ProviderConfig;

use crate::assets::Assets;
use crate::chat::Chat;
use crate::http::ImageHttp;
use crate::workspace::Workspace;

fn main() {
    // The local offset can only be read while the process is single-threaded.
    theme::remember_local_offset();
    let env = |name: &str| std::env::var(name).ok();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("the operating system can start threads");

    let workspace = settings::opened(settings::load(), settings::native_room(env));
    let mut config = ProviderConfig::from_env(env, &settings::guest_nickname());
    config.third_party_emotes = workspace.third_party_emotes;
    let store = ChatStore::new(
        workspace.max_messages_per_channel,
        &workspace.highlight_keywords,
    );
    let images = Arc::new(ImageHttp::new(
        config.http.clone(),
        runtime.handle().clone(),
    ));

    Application::new()
        .with_assets(Assets)
        .with_http_client(images)
        .run(move |cx: &mut App| {
            text_input::bind_keys(cx);
            workspace::bind_keys(cx);
            cx.on_window_closed(|cx| cx.quit()).detach();

            let chat = cx.new(|cx| Chat::new(store, config, runtime, cx));
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1120.), px(760.)),
                    cx,
                ))),
                window_min_size: Some(size(px(360.), px(320.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Linker-chat".into()),
                    ..TitlebarOptions::default()
                }),
                app_id: Some("linker-chat-desktop".into()),
                ..WindowOptions::default()
            };
            let opened = cx.open_window(options, |window, cx| {
                cx.new(|cx| Workspace::new(workspace, chat, window, cx))
            });
            if let Err(error) = opened {
                eprintln!("Could not open a window: {error:#}");
                cx.quit();
            }
            cx.activate(true);
        });
}
