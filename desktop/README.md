# Linker-chat desktop

A Rust and GPUI chat client that puts Linker-chat, Twitch, Kick, and YouTube
Live side by side or in one merged feed.

## Run

```bash
cd desktop
cargo run --release -p linker-chat-desktop
```

With nothing set, it joins room `main` at `ws://127.0.0.1:8080/ws` as a guest.
To join as the broadcaster of a local server:

```bash
LINKER_CHAT_TOKEN=dev-admin-token cargo run --release -p linker-chat-desktop
```

On Linux it needs a Vulkan driver and the development files for
`xkbcommon`, `xkbcommon-x11`, `xcb`, `fontconfig`, and `freetype`.

## Settings

| Variable | Meaning |
| --- | --- |
| `LINKER_CHAT_SERVER` | Native server. Default `ws://127.0.0.1:8080/ws`. |
| `LINKER_CHAT_ROOM` | Native room. Default `main`. |
| `LINKER_CHAT_TOKEN` | Personal or admin API token. |
| `LINKER_CHAT_DISPLAY_NAME` | Name to use with the token. |
| `LINKER_CHAT_JWT` | Clerk session token, if there is no API token. |
| `LINKER_CHAT_NICKNAME` | Guest nickname, if there is neither. |
| `LINKER_CHAT_TWITCH_LOGIN`, `LINKER_CHAT_TWITCH_TOKEN` | Twitch login and OAuth token with `chat:read chat:edit`. Without both, Twitch is read-only. |
| `LINKER_CHAT_KICK_TOKEN` | Kick API token with `chat:write`. Without it, Kick is read-only. |
| `LINKER_CHAT_YOUTUBE_API_KEY` | Reads YouTube Live chat. |
| `LINKER_CHAT_YOUTUBE_TOKEN` | Google OAuth token. Reads and sends. |
| `LINKER_CHAT_CONFIG` | Layout file. Default `~/.config/linker-chat/workspace.toml`. |

The layout file also holds `highlight_keywords`, `pause_on_hover`,
`third_party_emotes`, and `max_messages_per_channel`. Tokens are never written
to it.

## Keys

| Key | Action |
| --- | --- |
| Ctrl+K | Connect a channel |
| Ctrl+T | New tab |
| Ctrl+Tab, Ctrl+Shift+Tab | Next or previous tab |
| Ctrl+W | Close the active split |
| Ctrl+F | Search the active split |
| Esc | Cancel a reply, the search, or the connect bar |
| Ctrl+Q | Quit |

## Layout

- `crates/chat-core` holds the chat types, emote tokenizer, and state reducer.
  It does no I/O.
- `crates/providers` holds one connector per platform.
- `crates/app-gpui` holds the window.

## Test

```bash
(cd ../server && go build -o bin/linker-chat-server ./cmd/linker-chat-server)
cargo test --workspace
```

The native tests run the real Go server. `LINKER_CHAT_SERVER_BIN` points them
at another build of it. To watch one live channel without the window:

```bash
cargo run -p providers --example probe -- twitch:somechannel 10
```
