export type ChatRole = 'broadcaster' | 'moderator' | 'vip' | 'member' | 'guest';

export interface ChatUser {
  id: string;
  username: string;
  display_name: string;
  avatar_url?: string;
  color: string;
  role?: ChatRole;
  is_guest: boolean;
  is_admin?: boolean;
}

export interface Emote {
  code: string;
  url: string;
  animated: boolean;
  provider?: 'native' | '7tv' | 'bttv' | 'ffz';
}

export interface RoomState {
  room: string;
  title: string;
  slow_mode_sec: number;
  allow_guests: boolean;
  viewer_count: number;
}

export interface ChatMessage {
  id: string;
  room: string;
  user: ChatUser;
  content: string;
  reply_to_id?: string;
  badges: string[];
  created_at: string;
  deleted?: boolean;
}

export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected' | 'reconnecting';

export interface Envelope<T = unknown> {
  type: string;
  req_id?: string;
  data?: T;
}

export interface AuthOKPayload {
  user: ChatUser;
  guest_token?: string;
}

export interface RoomJoinedPayload {
  room: string;
  state: RoomState;
  role: ChatRole;
  viewer_count: number;
  emotes: Emote[];
  recent_messages: ChatMessage[];
}

export interface MessageDeletedPayload {
  room: string;
  message_id: string;
  actor_id: string;
}

export interface UserModeratedPayload {
  room: string;
  action: string;
  target_user_id?: string;
  target_username?: string;
  duration_sec?: number;
  reason?: string;
  actor_id: string;
}

export interface ChatClearedPayload {
  room: string;
  actor_id: string;
}

export interface SystemPayload {
  room?: string;
  text: string;
}

export interface ErrorPayload {
  code: string;
  message: string;
}

export interface EmoteOptions {
  sevenTvGlobal?: boolean;
  sevenTvEmoteSetId?: string;
  bttvGlobal?: boolean;
}

export type MessageFragment =
  | { kind: 'text'; text: string }
  | { kind: 'emote'; emote: Emote }
  | { kind: 'link'; url: string }
  | { kind: 'mention'; username: string; raw: string };

export interface LinkerChatClientOptions {
  /** WebSocket URL, e.g. "ws://localhost:8080/ws" or "wss://chat.example.com/ws" */
  serverUrl: string;
  /** Room slug to join immediately upon connect */
  room: string;
  /** Async token getter for Linker Clerk (e.g., () => clerk.session?.getToken({ template: 'linker-chat' })) */
  getToken?: () => Promise<string | null | undefined> | string | null | undefined;
  /** Optional personal broadcaster/moderator API token */
  authToken?: string;
  /** Optional display name override when using authToken */
  displayName?: string;
  /** Maximum messages kept in client ring buffer */
  maxMessages?: number;
}
