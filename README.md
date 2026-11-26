# Linker-chat

Open-source, Chatterino-inspired multi-platform streaming chat ecosystem.

**Linker-chat** combines:
1. **Native Chat Server (`server/`)** — A tiny, single-binary **Go + SQLite** WebSocket chat server with **Linker Clerk** (OIDC/JWKS) login, optional guest nicknames, room moderation, custom emotes, and personal API tokens.
2. **Embeddable Website Chat (`web/`)** — A **TypeScript SDK** (`@linker-chat/client`) and **SvelteKit / Svelte 5 component** (`@linker-chat/svelte`) designed to sit right next to a live stream, plus an iframe `/embed/[room]` route.
3. **Desktop Multi-Chat Client (`desktop/`)** — A native **Rust + GPUI** Chatterino-style client with multi-tab / multi-split panes, merged feeds, 7TV/BTTV/FFZ emotes, and connectors for **Native (Linker-chat)**, **Twitch**, **Kick**, and **YouTube Live**.

## Repository Structure

```text
linker-chat/
├── protocol/            # Shared WebSocket JSON protocol spec & schema
├── server/              # Go + SQLite native chat server (Clerk JWKS + guests + mod tools)
├── web/                 # pnpm workspace: @linker-chat/client, @linker-chat/svelte, SvelteKit demo
└── desktop/             # Cargo workspace: chat-core, platform connectors, emote engine, GPUI app
```

## Quick Start

### 1. Run the Go Native Chat Server
```bash
cd server
LINKER_CHAT_ADMIN_TOKEN=dev-admin-token go run ./cmd/linker-chat-server
```
To enable **Linker Clerk** JWT verification:
```bash
LINKER_CHAT_CLERK_JWKS_URL="https://<your-clerk-domain>/.well-known/jwks.json" \
LINKER_CHAT_CLERK_ISSUER="https://<your-clerk-domain>" \
LINKER_CHAT_ADMIN_TOKEN="change-me" \
go run ./cmd/linker-chat-server
```

### 2. Run the SvelteKit Demo Site (Stream + Embedded Chat)
```bash
cd web
pnpm install
pnpm --filter @linker-chat/sveltekit-demo dev
```
Open `http://localhost:5173` (or `/embed/main` for an iframe-ready embed).

### 3. Run the Rust + GPUI Desktop Client
```bash
cd desktop
cargo run -p linker-chat-desktop
```

## License

MIT — see [LICENSE](./LICENSE).
