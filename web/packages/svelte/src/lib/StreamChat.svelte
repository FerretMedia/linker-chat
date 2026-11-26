<script lang="ts">
  import { onMount, tick } from 'svelte';
  import {
    LinkerChatClient,
    loadThirdPartyEmotes,
    tokenizeMessage,
    type ChatMessage,
    type ClientSnapshot,
    type Emote,
    type EmoteOptions,
  } from '@linker-chat/client';

  interface Props {
    serverUrl: string;
    room?: string;
    /** Clerk / OIDC token getter, e.g. () => clerk.session?.getToken({ template: 'linker-chat' }) */
    getToken?: () => Promise<string | null | undefined> | string | null | undefined;
    /** Optional personal broadcaster/moderator token */
    authToken?: string;
    displayName?: string;
    allowGuests?: boolean;
    theme?: 'dark' | 'light';
    showHeader?: boolean;
    /** Full-bleed: drops the outer border and radius, for iframes and docked panels */
    embedded?: boolean;
    placeholder?: string;
    emotes?: EmoteOptions;
    maxMessages?: number;
  }

  let {
    serverUrl,
    room = 'main',
    getToken,
    authToken,
    displayName,
    allowGuests = true,
    theme = 'dark',
    showHeader = true,
    embedded = false,
    placeholder = 'Send a message...',
    emotes = { sevenTvGlobal: true, bttvGlobal: true },
    maxMessages = 400,
  }: Props = $props();

  let client: LinkerChatClient | null = null;
  let snapshot = $state<ClientSnapshot>({
    status: 'disconnected',
    room: 'main',
    roomState: null,
    user: null,
    role: 'guest',
    messages: [],
    nativeEmotes: [],
    lastError: null,
    systemNotices: [],
  });

  let thirdPartyEmotes = $state<Emote[]>([]);
  let draft = $state('');
  let guestInput = $state('');
  let replyTo = $state<ChatMessage | null>(null);
  let paused = $state(false);
  let unreadWhilePaused = $state(0);
  let scrollEl: HTMLDivElement | undefined = $state();

  const emoteMap = $derived.by(() => {
    const map = new Map<string, Emote>();
    for (const e of thirdPartyEmotes) map.set(e.code, e);
    for (const e of snapshot.nativeEmotes) map.set(e.code, e);
    return map;
  });

  const canModerate = $derived(
    snapshot.role === 'broadcaster' || snapshot.role === 'moderator' || Boolean(snapshot.user?.is_admin)
  );

  const guestsAllowed = $derived(
    allowGuests && (snapshot.roomState ? snapshot.roomState.allow_guests : true)
  );

  onMount(() => {
    const c = new LinkerChatClient({
      serverUrl,
      room,
      getToken,
      authToken,
      displayName,
      maxMessages,
    });
    client = c;

    let prevLen = 0;
    const unsub = c.subscribe(async (next) => {
      const added = next.messages.length - prevLen;
      prevLen = next.messages.length;
      snapshot = next;
      if (added > 0) {
        if (paused) {
          unreadWhilePaused += added;
        } else {
          await tick();
          scrollToBottom();
        }
      }
    });

    c.connect();
    loadThirdPartyEmotes(emotes).then((loaded) => {
      thirdPartyEmotes = loaded;
    });

    return () => {
      unsub();
      c.disconnect();
      client = null;
    };
  });

  $effect(() => {
    if (client && room && room !== snapshot.room) {
      client.setRoom(room);
    }
  });

  function scrollToBottom() {
    if (!scrollEl) return;
    scrollEl.scrollTop = scrollEl.scrollHeight;
    paused = false;
    unreadWhilePaused = 0;
  }

  function handleScroll() {
    if (!scrollEl) return;
    const distFromBottom = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight;
    const isAtBottom = distFromBottom < 36;
    paused = !isAtBottom;
    if (isAtBottom) unreadWhilePaused = 0;
  }

  function handleSend(e: SubmitEvent) {
    e.preventDefault();
    if (!client || !draft.trim()) return;
    client.sendMessage(draft, replyTo?.id);
    draft = '';
    replyTo = null;
  }

  function handleGuestSubmit(e: SubmitEvent) {
    e.preventDefault();
    if (!client || guestInput.trim().length < 2) return;
    client.loginAsGuest(guestInput.trim());
    guestInput = '';
  }

  function formatTime(iso: string): string {
    const d = new Date(iso);
    return Number.isNaN(d.getTime())
      ? ''
      : d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }

  function findReplyMessage(id?: string): ChatMessage | undefined {
    if (!id) return undefined;
    return snapshot.messages.find((m) => m.id === id);
  }
</script>

{#snippet identity(user: NonNullable<ClientSnapshot['user']>)}
  <span class="lc-identity" title="Chatting as {user.display_name}">
    <strong class="lc-author" style:--lc-user={user.color}>{user.display_name}</strong>
    <span class="lc-pill">{snapshot.role}</span>
  </span>
{/snippet}

<div class="lc-root" class:lc-embedded={embedded} data-theme={theme}>
  {#if showHeader}
    <header class="lc-header">
      <div class="lc-header-left">
        <span class="lc-dot" data-status={snapshot.status}></span>
        <span class="lc-title">#{snapshot.roomState?.title || snapshot.room}</span>
        {#if snapshot.roomState?.slow_mode_sec}
          <span class="lc-pill">Slow {snapshot.roomState.slow_mode_sec}s</span>
        {/if}
      </div>
      <div class="lc-header-right">
        {#if snapshot.user}
          {@render identity(snapshot.user)}
        {/if}
        <span class="lc-viewers" title="Connected chatters">
          {snapshot.roomState?.viewer_count ?? 0} live
        </span>
      </div>
    </header>
  {/if}

  <div class="lc-messages" bind:this={scrollEl} onscroll={handleScroll}>
    {#if snapshot.messages.length === 0}
      <div class="lc-empty">Welcome to #{snapshot.room}. Say hi!</div>
    {/if}

    {#each snapshot.messages as msg (msg.id)}
      {@const parent = findReplyMessage(msg.reply_to_id)}
      <div class="lc-msg" class:lc-deleted={msg.deleted}>
        {#if parent}
          <div class="lc-reply-ref">
            ↳ Replying to @{parent.user.display_name}: {parent.content.slice(0, 60)}
          </div>
        {/if}
        <div class="lc-msg-line">
          <span class="lc-time">{formatTime(msg.created_at)}</span>

          {#each msg.badges as badge}
            <span class="lc-badge" data-badge={badge}>{badge}</span>
          {/each}

          <span class="lc-author" style:--lc-user={msg.user.color}>{msg.user.display_name}</span><span class="lc-colon">:</span>

          <span class="lc-content">
            {#each tokenizeMessage(msg.content, emoteMap) as frag}
              {#if frag.kind === 'emote'}
                <img
                  class="lc-emote"
                  src={frag.emote.url}
                  alt={frag.emote.code}
                  title={frag.emote.code}
                  loading="lazy"
                />
              {:else if frag.kind === 'link'}
                <a class="lc-link" href={frag.url} target="_blank" rel="noopener noreferrer">{frag.url}</a>
              {:else if frag.kind === 'mention'}
                <span class="lc-mention">{frag.raw}</span>
              {:else}
                {frag.text}
              {/if}
            {/each}
          </span>

          {#if !msg.deleted}
            <span class="lc-actions">
              <button type="button" class="lc-act-btn" title="Reply" onclick={() => (replyTo = msg)}>↩</button>
              {#if canModerate}
                <button
                  type="button"
                  class="lc-act-btn"
                  title="Delete message"
                  onclick={() => client?.moderate({ action: 'delete', message_id: msg.id })}
                >✕</button>
                <button
                  type="button"
                  class="lc-act-btn"
                  title="Timeout 10m"
                  onclick={() =>
                    client?.moderate({
                      action: 'timeout',
                      target_user_id: msg.user.id,
                      target_username: msg.user.username,
                      duration_sec: 600,
                    })}
                >⏱</button>
                <button
                  type="button"
                  class="lc-act-btn lc-danger"
                  title="Ban user"
                  onclick={() =>
                    client?.moderate({
                      action: 'ban',
                      target_user_id: msg.user.id,
                      target_username: msg.user.username,
                    })}
                >⊘</button>
              {/if}
            </span>
          {/if}
        </div>
      </div>
    {/each}

    {#each snapshot.systemNotices.slice(-3) as notice}
      <div class="lc-system">{notice}</div>
    {/each}
  </div>

  {#if paused}
    <button type="button" class="lc-pause-pill" onclick={scrollToBottom}>
      Chat paused {unreadWhilePaused > 0 ? `(${unreadWhilePaused} new)` : ''} — Jump to latest
    </button>
  {/if}

  {#if snapshot.lastError}
    <div class="lc-error">{snapshot.lastError.message}</div>
  {/if}

  <footer class="lc-footer">
    {#if replyTo}
      <div class="lc-reply-bar">
        <span>Replying to <strong>@{replyTo.user.display_name}</strong></span>
        <button type="button" class="lc-clear-reply" onclick={() => (replyTo = null)}>✕</button>
      </div>
    {/if}

    {#if snapshot.user}
      {#if !showHeader}
        <div class="lc-identity-bar">{@render identity(snapshot.user)}</div>
      {/if}
      <form class="lc-form" onsubmit={handleSend}>
        <input
          class="lc-input"
          type="text"
          bind:value={draft}
          placeholder={placeholder}
          maxlength={500}
          autocomplete="off"
        />
        <button class="lc-send" type="submit" disabled={!draft.trim() || snapshot.status !== 'connected'}>
          Send
        </button>
      </form>
    {:else if guestsAllowed}
      <form class="lc-form" onsubmit={handleGuestSubmit}>
        <input
          class="lc-input"
          type="text"
          bind:value={guestInput}
          placeholder="Pick a guest nickname to chat..."
          minlength={2}
          maxlength={24}
        />
        <button class="lc-send" type="submit" disabled={guestInput.trim().length < 2}>Join</button>
      </form>
    {:else}
      <div class="lc-login-hint">Sign in with Linker Clerk to join the conversation.</div>
    {/if}
  </footer>
</div>

<style>
  .lc-root {
    --lc-bg: #09090b;
    --lc-surface: #111115;
    --lc-raised: #16161c;
    --lc-border: #26262c;
    --lc-text: #fafafa;
    --lc-muted: #85858f;
    --lc-secondary: #a1a1aa;
    --lc-accent: #4545e6;
    --lc-accent-hover: #5656f0;
    --lc-accent-bright: #6d6dff;
    --lc-crimson: #9f1d8e;
    --lc-crimson-bright: #f52bd0;
    --lc-rec: #ff3131;
    --lc-good: #34d399;
    --lc-warn: #f59e0b;
    --lc-mention: #a5b4fc;
    --lc-error-text: #fca5a5;
    --lc-font: system-ui, -apple-system, 'Segoe UI', sans-serif;

    color-scheme: dark;
    position: relative;
    display: flex;
    flex-direction: column;
    box-sizing: border-box;
    height: 100%;
    min-height: 320px;
    background: var(--lc-bg);
    color: var(--lc-text);
    font-family: var(--lc-font);
    font-size: 12.5px;
    line-height: 1.38;
    border: 1px solid var(--lc-border);
    border-radius: 8px 8px 0 0;
    overflow: hidden;
  }

  .lc-root[data-theme='light'] {
    --lc-bg: #fafafa;
    --lc-surface: #f4f4f6;
    --lc-raised: #ebebef;
    --lc-border: #e4e4e7;
    --lc-text: #09090b;
    --lc-muted: #71717a;
    --lc-secondary: #52525b;
    --lc-accent-bright: #4545e6;
    --lc-mention: #4545e6;
    --lc-error-text: #b91c1c;

    color-scheme: light;
  }

  .lc-root.lc-embedded {
    border: 0;
    border-radius: 0;
  }

  .lc-root button,
  .lc-root input {
    font-family: inherit;
  }
  .lc-root button:focus-visible {
    outline: 2px solid var(--lc-accent-bright);
    outline-offset: 1px;
  }
  .lc-root .lc-send:focus-visible,
  .lc-root .lc-pause-pill:focus-visible {
    outline-color: #fff;
    outline-offset: -4px;
  }

  .lc-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 7px 10px;
    background: var(--lc-surface);
    border-bottom: 1px solid var(--lc-border);
    flex-shrink: 0;
  }

  .lc-header-left,
  .lc-header-right,
  .lc-identity {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .lc-header-right {
    gap: 8px;
    flex-shrink: 0;
  }

  .lc-identity {
    font-size: 11px;
  }
  .lc-identity .lc-author {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .lc-dot {
    width: 6px;
    height: 6px;
    flex-shrink: 0;
    border-radius: 999px;
    background: var(--lc-muted);
  }
  .lc-dot[data-status='connected'] {
    background: var(--lc-good);
  }
  .lc-dot[data-status='connecting'],
  .lc-dot[data-status='reconnecting'] {
    background: var(--lc-warn);
  }

  .lc-title {
    font-weight: 600;
    letter-spacing: -0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .lc-pill {
    flex-shrink: 0;
    font-size: 10px;
    line-height: 1.3;
    padding: 0 4px;
    border: 1px solid var(--lc-border);
    border-radius: 3px;
    background: var(--lc-raised);
    color: var(--lc-secondary);
  }

  .lc-viewers {
    flex-shrink: 0;
    font-size: 11px;
    color: var(--lc-secondary);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .lc-messages {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .lc-empty {
    margin: auto;
    color: var(--lc-muted);
  }

  .lc-msg {
    position: relative;
    padding: 2px 6px;
    border-radius: 4px;
    overflow-wrap: anywhere;
  }
  .lc-msg:hover {
    background: var(--lc-raised);
  }
  .lc-deleted {
    opacity: 0.5;
    font-style: italic;
  }

  .lc-reply-ref {
    font-size: 11px;
    color: var(--lc-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .lc-time {
    font-size: 10.5px;
    color: var(--lc-muted);
    margin-right: 1px;
    font-variant-numeric: tabular-nums;
  }

  .lc-badge {
    display: inline-block;
    font-size: 9.5px;
    line-height: 1.2;
    font-weight: 700;
    letter-spacing: 0.02em;
    text-transform: uppercase;
    padding: 1px 4px;
    border-radius: 3px;
    vertical-align: 1px;
    background: #26262c;
    color: #a1a1aa;
  }
  .lc-badge[data-badge='broadcaster'] {
    background: var(--lc-rec);
    color: #09090b;
  }
  .lc-badge[data-badge='moderator'] {
    background: #16a34a;
    color: #09090b;
  }
  .lc-badge[data-badge='vip'] {
    background: var(--lc-crimson);
    box-shadow: inset 0 0 0 1px var(--lc-crimson-bright);
    color: #fff;
  }

  .lc-author {
    font-weight: 600;
    color: var(--lc-user);
  }
  /* Chatter colors are picked for dark; darken them to stay readable on paper. */
  .lc-root[data-theme='light'] .lc-author {
    color: color-mix(in srgb, var(--lc-user) 48%, #09090b);
  }
  .lc-colon {
    color: var(--lc-muted);
  }

  .lc-emote {
    height: 19px;
    width: auto;
    vertical-align: middle;
    margin: -3px 1px;
    display: inline-block;
  }

  .lc-link {
    color: var(--lc-accent-bright);
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .lc-mention {
    background: rgb(69 69 230 / 22%);
    color: var(--lc-mention);
    border-radius: 3px;
    padding: 0 2px;
    font-weight: 600;
  }
  /* Paper needs a lighter tint for the accent text to pass 4.5:1. */
  .lc-root[data-theme='light'] .lc-mention {
    background: rgb(69 69 230 / 12%);
  }

  .lc-actions {
    display: none;
    position: absolute;
    right: 6px;
    top: 1px;
    background: var(--lc-surface);
    border: 1px solid var(--lc-border);
    border-radius: 5px;
    padding: 1px 2px;
  }
  .lc-msg:hover .lc-actions {
    display: inline-flex;
  }
  .lc-act-btn {
    border: 0;
    background: transparent;
    color: var(--lc-secondary);
    cursor: pointer;
    font-size: 11px;
    line-height: 1.3;
    padding: 2px 6px;
    border-radius: 3px;
  }
  .lc-act-btn:hover {
    color: var(--lc-text);
    background: var(--lc-border);
  }
  .lc-act-btn.lc-danger:hover {
    color: var(--lc-rec);
  }

  .lc-system {
    font-size: 11px;
    color: var(--lc-secondary);
    padding: 2px 8px 2px 6px;
    border-left: 2px solid var(--lc-accent);
  }

  /* Negative top margin cancels the pill's height, so it floats over the
     list 8px above whatever follows (error banner or composer). */
  .lc-pause-pill {
    position: relative;
    z-index: 1;
    flex-shrink: 0;
    align-self: center;
    max-width: calc(100% - 16px);
    height: 24px;
    margin: -32px 0 8px;
    background: var(--lc-accent);
    color: #fff;
    border: 0;
    border-radius: 999px;
    padding: 0 12px;
    font-size: 11px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    cursor: pointer;
    box-shadow: 0 4px 14px rgb(0 0 0 / 35%);
  }
  .lc-pause-pill:hover {
    background: var(--lc-accent-hover);
  }

  .lc-error {
    padding: 5px 10px;
    background: rgb(255 49 49 / 10%);
    color: var(--lc-error-text);
    font-size: 11.5px;
    border-top: 1px solid rgb(255 49 49 / 30%);
    flex-shrink: 0;
  }

  .lc-footer {
    padding: 0;
    background: var(--lc-surface);
    border-top: 1px solid var(--lc-border);
    flex-shrink: 0;
  }

  .lc-reply-bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 11px;
    color: var(--lc-secondary);
    padding: 4px 8px;
    border-bottom: 1px solid var(--lc-border);
  }
  .lc-clear-reply {
    border: 0;
    background: transparent;
    color: inherit;
    font-size: 11px;
    padding: 0 4px;
    cursor: pointer;
  }

  .lc-form {
    display: flex;
    gap: 0;
    align-items: stretch;
  }

  .lc-input {
    flex: 1;
    min-width: 0;
    box-sizing: border-box;
    background: var(--lc-bg);
    color: var(--lc-text);
    border: 0;
    border-radius: 0;
    padding: 8px 10px;
    font-size: 12.5px;
    outline: none;
  }
  .lc-input::placeholder {
    color: var(--lc-muted);
    opacity: 1;
  }
  .lc-input:focus {
    box-shadow: inset 0 0 0 1px var(--lc-accent-bright);
  }

  .lc-send {
    background: var(--lc-accent);
    color: #fff;
    border: 0;
    border-radius: 0;
    padding: 0 14px;
    font-weight: 600;
    font-size: 12px;
    cursor: pointer;
  }
  .lc-send:hover:not(:disabled) {
    background: var(--lc-accent-hover);
  }
  .lc-send:disabled {
    background: var(--lc-raised);
    box-shadow: inset 1px 0 0 var(--lc-border);
    color: var(--lc-muted);
    cursor: not-allowed;
  }

  .lc-identity-bar {
    padding: 3px 10px;
    border-bottom: 1px solid var(--lc-border);
  }

  .lc-login-hint {
    font-size: 12px;
    color: var(--lc-secondary);
    text-align: center;
    padding: 8px 10px;
  }
</style>
