//! Prints a channel's events for a few seconds. Reads credentials from the
//! environment like the app does.
//!
//!     cargo run -p providers --example probe -- twitch:somechannel 10

use std::time::Duration;

use providers::{ProviderConfig, ReconnectPolicy};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let channel = args
        .next()
        .ok_or("usage: probe platform:channel [seconds]")?;
    let seconds = args.next().map_or(Ok(10), |n| n.parse())?;

    let config = ProviderConfig::from_env(|name| std::env::var(name).ok(), "probe");
    let globals = providers::fetch_global_emotes(&config.http).await;
    println!("{} global emotes", globals.len());

    let (sender, mut events) = mpsc::unbounded_channel();
    let provider = providers::connect(channel.parse()?, &config);
    let _handle = providers::spawn(provider, sender, ReconnectPolicy::default());
    let print = async {
        while let Some(event) = events.recv().await {
            match event.event {
                chat_core::ChatEvent::Emotes(emotes) => println!("{} emotes", emotes.len()),
                event => println!("{event:?}"),
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(seconds), print).await;
    Ok(())
}
