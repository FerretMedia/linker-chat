import type {
  AuthOKPayload,
  ChatClearedPayload,
  ChatMessage,
  ChatRole,
  ChatUser,
  ConnectionStatus,
  Emote,
  Envelope,
  ErrorPayload,
  LinkerChatClientOptions,
  MessageDeletedPayload,
  RoomJoinedPayload,
  RoomState,
  SystemPayload,
  UserModeratedPayload,
} from './types.js';

export interface ClientSnapshot {
  status: ConnectionStatus;
  room: string;
  roomState: RoomState | null;
  user: ChatUser | null;
  role: ChatRole;
  messages: ChatMessage[];
  nativeEmotes: Emote[];
  lastError: ErrorPayload | null;
  systemNotices: string[];
}

export type SnapshotListener = (snapshot: ClientSnapshot) => void;

const GUEST_TOKEN_KEY = 'linker_chat_guest_token';
const GUEST_NICK_KEY = 'linker_chat_guest_nick';

export class LinkerChatClient {
  private opts: LinkerChatClientOptions;
  private ws: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private reconnectAttempt = 0;
  private destroyed = false;
  private listeners = new Set<SnapshotListener>();
  private reqCounter = 0;
  private guestNickname: string | null = null;

  private state: ClientSnapshot;

  constructor(opts: LinkerChatClientOptions) {
    this.opts = opts;
    this.state = {
      status: 'disconnected',
      room: opts.room,
      roomState: null,
      user: null,
      role: 'guest',
      messages: [],
      nativeEmotes: [],
      lastError: null,
      systemNotices: [],
    };
  }

  getSnapshot(): ClientSnapshot {
    return this.state;
  }

  subscribe(listener: SnapshotListener): () => void {
    this.listeners.add(listener);
    listener(this.state);
    return () => {
      this.listeners.delete(listener);
    };
  }

  connect(): void {
    if (this.destroyed) return;
    this.clearTimers();
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }

    const nextStatus: ConnectionStatus = this.reconnectAttempt > 0 ? 'reconnecting' : 'connecting';
    this.patch({ status: nextStatus, lastError: null });

    const ws = new WebSocket(this.opts.serverUrl);
    this.ws = ws;

    ws.onopen = async () => {
      this.reconnectAttempt = 0;
      this.patch({ status: 'connected' });
      this.startPing();
      await this.authenticateCurrentSession();
      this.send('join', { room: this.state.room });
    };

    ws.onmessage = (evt) => {
      try {
        const env = JSON.parse(String(evt.data)) as Envelope;
        this.handleEnvelope(env);
      } catch {
        // Ignore malformed frames
      }
    };

    ws.onclose = () => {
      this.stopPing();
      if (this.destroyed) {
        this.patch({ status: 'disconnected' });
        return;
      }
      this.scheduleReconnect();
    };

    ws.onerror = () => {
      // onclose handles backoff reconnect
    };
  }

  disconnect(): void {
    this.destroyed = true;
    this.clearTimers();
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
    this.patch({ status: 'disconnected' });
  }

  setRoom(room: string): void {
    const clean = room.trim().toLowerCase() || 'main';
    if (clean === this.state.room) return;
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.send('leave', { room: this.state.room });
    }
    this.patch({ room: clean, messages: [], roomState: null });
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.send('join', { room: clean });
    }
  }

  async refreshAuth(): Promise<void> {
    if (this.ws?.readyState !== WebSocket.OPEN) return;
    await this.authenticateCurrentSession();
    this.send('join', { room: this.state.room });
  }

  loginAsGuest(nickname: string): void {
    const clean = nickname.trim();
    if (clean.length < 2) return;
    this.guestNickname = clean;
    this.saveLocal(GUEST_NICK_KEY, clean);
    const savedToken = this.readLocal(GUEST_TOKEN_KEY);
    this.send('auth_guest', { nickname: clean, guest_token: savedToken || undefined });
  }

  sendMessage(content: string, replyToId?: string): void {
    const trimmed = content.trim();
    if (!trimmed) return;
    this.send('send_message', {
      room: this.state.room,
      content: trimmed,
      reply_to_id: replyToId || undefined,
    });
  }

  moderate(payload: {
    action: 'delete' | 'timeout' | 'ban' | 'unban' | 'clear' | 'set_role';
    message_id?: string;
    target_user_id?: string;
    target_username?: string;
    duration_sec?: number;
    role?: ChatRole;
    reason?: string;
  }): void {
    this.send('moderate', {
      room: this.state.room,
      ...payload,
    });
  }

  private async authenticateCurrentSession(): Promise<void> {
    if (this.opts.authToken) {
      this.send('auth_token', {
        token: this.opts.authToken,
        display_name: this.opts.displayName,
      });
      return;
    }

    if (this.opts.getToken) {
      try {
        const jwt = await this.opts.getToken();
        if (jwt) {
          this.send('auth', { token: jwt });
          return;
        }
      } catch {
        // Fall through to saved guest session if Clerk token unavailable
      }
    }

    const savedNick = this.guestNickname || this.readLocal(GUEST_NICK_KEY);
    const savedToken = this.readLocal(GUEST_TOKEN_KEY);
    if (savedNick) {
      this.send('auth_guest', { nickname: savedNick, guest_token: savedToken || undefined });
    }
  }

  private handleEnvelope(env: Envelope): void {
    const maxMessages = this.opts.maxMessages ?? 400;

    switch (env.type) {
      case 'auth_ok': {
        const data = env.data as AuthOKPayload;
        if (data.guest_token) {
          this.saveLocal(GUEST_TOKEN_KEY, data.guest_token);
        }
        this.patch({
          user: data.user,
          role: data.user.role ?? (data.user.is_guest ? 'guest' : 'member'),
          lastError: null,
        });
        break;
      }
      case 'room_joined': {
        const data = env.data as RoomJoinedPayload;
        this.patch({
          room: data.room,
          roomState: data.state,
          role: data.role,
          nativeEmotes: (data.emotes ?? []).map((e) => ({ ...e, provider: 'native' as const })),
          messages: (data.recent_messages ?? []).slice(-maxMessages),
          lastError: null,
        });
        break;
      }
      case 'room_state': {
        const data = env.data as RoomState;
        if (data.room === this.state.room) {
          this.patch({ roomState: data });
        }
        break;
      }
      case 'message': {
        const msg = env.data as ChatMessage;
        if (msg.room !== this.state.room) return;
        const next = [...this.state.messages, msg];
        if (next.length > maxMessages) {
          next.splice(0, next.length - maxMessages);
        }
        this.patch({ messages: next, lastError: null });
        break;
      }
      case 'message_deleted': {
        const data = env.data as MessageDeletedPayload;
        this.patch({
          messages: this.state.messages.map((m) =>
            m.id === data.message_id ? { ...m, deleted: true, content: '<message deleted>' } : m
          ),
        });
        break;
      }
      case 'user_moderated': {
        const data = env.data as UserModeratedPayload;
        const target = data.target_username || data.target_user_id || 'user';
        const notice =
          data.action === 'timeout'
            ? `${target} was timed out for ${data.duration_sec ?? 600}s${data.reason ? `: ${data.reason}` : ''}`
            : data.action === 'ban'
              ? `${target} was banned${data.reason ? `: ${data.reason}` : ''}`
              : `${target} was unbanned`;
        this.patch({
          systemNotices: [...this.state.systemNotices.slice(-9), notice],
        });
        break;
      }
      case 'chat_cleared': {
        const data = env.data as ChatClearedPayload;
        if (data.room === this.state.room) {
          this.patch({
            messages: [],
            systemNotices: [...this.state.systemNotices.slice(-9), 'Chat was cleared by a moderator.'],
          });
        }
        break;
      }
      case 'system': {
        const data = env.data as SystemPayload;
        this.patch({
          systemNotices: [...this.state.systemNotices.slice(-9), data.text],
        });
        break;
      }
      case 'error': {
        const data = env.data as ErrorPayload;
        this.patch({ lastError: data });
        break;
      }
    }
  }

  private send(type: string, data: unknown): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
    const req_id = `c${++this.reqCounter}`;
    this.ws.send(JSON.stringify({ type, req_id, data }));
  }

  private startPing(): void {
    this.stopPing();
    this.pingTimer = setInterval(() => {
      this.send('ping', { ts: Date.now() });
    }, 25000);
  }

  private stopPing(): void {
    if (this.pingTimer) {
      clearInterval(this.pingTimer);
      this.pingTimer = null;
    }
  }

  private scheduleReconnect(): void {
    this.reconnectAttempt += 1;
    const delay = Math.min(1000 * 2 ** (this.reconnectAttempt - 1), 15000);
    this.patch({ status: 'reconnecting' });
    this.reconnectTimer = setTimeout(() => this.connect(), delay);
  }

  private clearTimers(): void {
    this.stopPing();
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  private patch(partial: Partial<ClientSnapshot>): void {
    this.state = { ...this.state, ...partial };
    for (const listener of this.listeners) {
      listener(this.state);
    }
  }

  private readLocal(key: string): string | null {
    try {
      return typeof localStorage !== 'undefined' ? localStorage.getItem(key) : null;
    } catch {
      return null;
    }
  }

  private saveLocal(key: string, val: string): void {
    try {
      if (typeof localStorage !== 'undefined') localStorage.setItem(key, val);
    } catch {
      // Ignore storage restrictions in sandboxed iframes
    }
  }
}
