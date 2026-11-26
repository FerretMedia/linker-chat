# Linker-chat Wire Protocol v1

All real-time communication between the **Go Native Chat Server**, the **SvelteKit Web Embed**, and the **Rust GPUI Desktop Client** uses UTF-8 JSON frames over WebSocket (`/ws`).

## Frame Envelope

Every frame has a `type` discriminator and optional `req_id` for correlating responses:

```json
{
  "type": "send_message",
  "req_id": "c1",
  "data": { ... }
}
```

---

## Authentication Flow

After opening `/ws`, a client **may** authenticate using one of three frames, or jump straight to `join` as a read-only lurker:

1. **Clerk / OIDC JWT (`auth`)** — Used by SvelteKit websites authenticated via Linker Clerk:
   ```json
   { "type": "auth", "data": { "token": "<clerk-jwt>" } }
   ```
   The server verifies the signature against `LINKER_CHAT_CLERK_JWKS_URL` (or `LINKER_CHAT_JWT_SECRET` in HMAC dev mode) and extracts `sub`, `username`/`preferred_username`/`name`, `image_url`/`picture`, and optional `color`.

2. **Guest Nickname (`auth_guest`)** — Used when a room permits guest chatting:
   ```json
   { "type": "auth_guest", "data": { "nickname": "ferret42", "guest_token": "<optional-resumed-token>" } }
   ```
   Returns a signed `guest_token` in `auth_ok` so refreshes preserve the guest identity.

3. **Personal API Token (`auth_token`)** — Used by the GPUI desktop client (broadcaster/mod) or bots:
   ```json
   { "type": "auth_token", "data": { "token": "<api-or-admin-token>", "display_name": "Broadcaster" } }
   ```

---

## Client -> Server Frames

| `type` | `data` fields | Notes |
| :--- | :--- | :--- |
| `auth` | `{ token }` | Clerk / OIDC / HMAC JWT login |
| `auth_guest` | `{ nickname, guest_token? }` | Guest identity (subject to room `allow_guests`) |
| `auth_token` | `{ token, display_name? }` | Broadcaster/mod personal token or admin token |
| `join` | `{ room }` | Subscribe to room messages & receive recent history + room state |
| `leave` | `{ room }` | Unsubscribe from a room |
| `send_message` | `{ room, content, reply_to_id? }` | Send chat message or slash command (`/timeout`, `/ban`, `/unban`, `/mod`, `/unmod`, `/vip`, `/slow`, `/clear`, `/guests`) |
| `moderate` | `{ room, action, target_user_id?, target_username?, message_id?, duration_sec?, reason? }` | `action`: `delete \| timeout \| ban \| unban \| clear \| set_role` |
| `ping` | `{ ts? }` | Keepalive ping |

---

## Server -> Client Frames

| `type` | `data` fields | Notes |
| :--- | :--- | :--- |
| `auth_ok` | `{ user, guest_token? }` | Authenticated identity (`id`, `username`, `display_name`, `avatar_url`, `color`, `is_guest`, `is_admin`) |
| `room_joined` | `{ room, state, role, viewer_count, emotes, recent_messages }` | Initial room snapshot |
| `room_state` | `{ room, slow_mode_sec, allow_guests, title, viewer_count }` | Broadcast when room settings or viewer count change |
| `message` | `{ id, room, user, content, reply_to_id?, badges, created_at }` | Live chat message |
| `message_deleted` | `{ room, message_id, actor_id }` | Single message removed by a mod |
| `user_moderated` | `{ room, action, target_user_id, target_username, duration_sec?, reason?, actor_id }` | Timeout / ban / unban notice |
| `chat_cleared` | `{ room, actor_id }` | Room chat history cleared |
| `system` | `{ room?, text }` | Ephemeral system notice to this connection |
| `error` | `{ code, message }` | Structured error (`unauthorized`, `banned`, `timed_out`, `slow_mode`, `rate_limited`, `guests_disabled`, `forbidden`, `bad_request`) |
| `pong` | `{ ts? }` | Reply to `ping` |

---

## REST Endpoints

- `GET /healthz` — Health check (`{ "status": "ok" }`)
- `GET /api/v1/rooms/{slug}` — Room info, viewer count, and native emotes
- `GET /api/v1/rooms/{slug}/messages?limit=50` — Recent messages
- `POST /api/v1/rooms/{slug}/emotes` — Register native room emote (broadcaster/mod/admin auth via `Authorization: Bearer <token>`)
- `POST /api/v1/tokens` — Mint a personal desktop API token using a valid Clerk JWT or admin token
