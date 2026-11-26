<script lang="ts">
  import { StreamChat } from '@linker-chat/svelte';

  let serverUrl = $state('ws://localhost:8080/ws');
  let room = $state('main');
  let clerkJwt = $state('');
  let theme = $state<'dark' | 'light'>('dark');

  // In a real Linker SvelteKit app using Clerk, replace this with:
  // const getToken = () => clerk.session?.getToken({ template: 'linker-chat' }) ?? null;
  const getToken = () => (clerkJwt.trim() ? clerkJwt.trim() : null);
</script>

<main class="layout">
  <section class="stage">
    <div class="player-shell">
      <header class="stage-header">
        <span class="brand">
          <svg viewBox="170 170 460 460" aria-hidden="true" focusable="false">
            {#each [{ fill: '#4545e6', offset: 14 }, { fill: '#9f1d8e', offset: -14 }, { fill: 'currentColor', offset: 0 }] as { fill, offset }}
              <g {fill} transform="translate({offset} {offset})">
                <rect x="198" y="192" width="154" height="275" rx="19" />
                <rect x="353" y="468" width="249" height="140" rx="19" />
              </g>
            {/each}
          </svg>
          Linker-chat
        </span>
        <span class="live">LIVE</span>
      </header>
      <div class="player-copy">
        <h1>Linker-chat Stream Embed Demo</h1>
        <p>
          Drop <code>&lt;StreamChat /&gt;</code> right beside your HLS / WebRTC / YouTube / Twitch player.
          Messages sync live with the Go server and the Rust GPUI desktop client.
        </p>
      </div>
    </div>

    <div class="controls">
      <label>
        <span>Server WS URL</span>
        <input bind:value={serverUrl} />
      </label>
      <label>
        <span>Room</span>
        <input bind:value={room} />
      </label>
      <label class="wide">
        <span>Linker Clerk JWT (optional — leave blank for guest mode)</span>
        <input bind:value={clerkJwt} placeholder="eyJhbGciOiJSUzI1NiIs..." />
      </label>
      <div class="row">
        <button type="button" onclick={() => (theme = theme === 'dark' ? 'light' : 'dark')}>
          Theme: {theme}
        </button>
        <a href={`/embed/${encodeURIComponent(room)}?server=${encodeURIComponent(serverUrl)}&theme=${theme}`}>
          Open standalone iframe embed →
        </a>
      </div>
    </div>
  </section>

  <aside class="chat-dock">
    {#key `${serverUrl}:${clerkJwt}`}
      <StreamChat
        {serverUrl}
        {room}
        {getToken}
        {theme}
        embedded={true}
        allowGuests={true}
        emotes={{ sevenTvGlobal: true, bttvGlobal: true }}
      />
    {/key}
  </aside>
</main>

<style>
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 350px;
    grid-template-rows: minmax(0, 1fr);
    height: 100dvh;
    gap: 0;
    padding: 0;
    box-sizing: border-box;
  }

  .stage {
    display: flex;
    flex-direction: column;
    gap: 0;
    min-width: 0;
    border-right: 1px solid #26262c;
  }

  .player-shell {
    flex: 1;
    min-height: 180px;
    display: flex;
    flex-direction: column;
    border: 0;
    border-radius: 0;
    border-bottom: 1px solid #26262c;
    overflow: hidden;
    background:
      radial-gradient(ellipse 55% 60% at 0% 0%, rgb(69 69 230 / 22%), transparent 70%),
      radial-gradient(ellipse 55% 60% at 100% 100%, rgb(159 29 142 / 20%), transparent 70%),
      #09090b;
  }

  .stage-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 34px;
    padding: 0 12px 0 0;
    box-sizing: border-box;
    border-bottom: 1px solid #26262c;
  }

  .brand {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 100%;
    padding: 0 12px;
    border-right: 1px solid #26262c;
    font-size: 13px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .brand svg {
    width: 20px;
    height: 20px;
  }

  .live {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: #ff3131;
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.08em;
  }

  .live::before {
    content: '';
    width: 7px;
    height: 7px;
    border-radius: 999px;
    background: currentColor;
  }

  .player-copy {
    margin: auto;
    padding: 16px;
    text-align: center;
  }

  .player-copy h1 {
    margin: 0 0 6px;
    font-size: 22px;
    font-weight: 600;
    line-height: 1.2;
    letter-spacing: -0.02em;
    text-wrap: balance;
  }

  .player-copy p {
    margin: 0;
    max-width: 520px;
    color: #a1a1aa;
    font-size: 13.5px;
    line-height: 1.5;
  }

  .player-copy code {
    color: #fafafa;
    font-size: 12.5px;
  }

  .controls {
    display: grid;
    grid-template-columns: minmax(0, 2fr) minmax(0, 1fr);
    gap: 0;
    background: #111115;
    border: 0;
    border-radius: 0;
    padding: 0;
  }

  .controls label {
    display: grid;
    gap: 0;
    padding: 6px 10px;
    border-bottom: 1px solid #26262c;
    font-size: 11px;
    color: #a1a1aa;
  }

  .controls label:first-child {
    border-right: 1px solid #26262c;
  }

  .controls label:focus-within {
    box-shadow: inset 0 0 0 1px #6d6dff;
  }

  .controls .wide,
  .row {
    grid-column: 1 / -1;
  }

  .controls input {
    width: 100%;
    box-sizing: border-box;
    background: transparent;
    color: #fafafa;
    border: 0;
    border-radius: 0;
    padding: 3px 0 0;
    font: inherit;
    font-size: 12.5px;
    outline: none;
  }

  .controls input::placeholder {
    color: #85858f;
    opacity: 1;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0;
    padding: 0;
    height: 32px;
  }

  .row button {
    height: 100%;
    background: #16161c;
    color: #fafafa;
    border: 0;
    border-right: 1px solid #26262c;
    border-radius: 0;
    padding: 0 12px;
    font: inherit;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
  }

  .row button:hover {
    background: #26262c;
  }

  .row a {
    padding: 0 12px;
    color: #a1a1aa;
    text-decoration: none;
    font-size: 12px;
  }

  .row a:hover {
    color: #fafafa;
  }

  .row button:focus-visible,
  .row a:focus-visible {
    outline: 2px solid #6d6dff;
    outline-offset: -2px;
  }

  .chat-dock {
    min-height: 0;
    height: 100%;
  }

  @media (max-width: 900px) {
    .layout {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: auto minmax(460px, 1fr);
    }

    .stage {
      border-right: 0;
      border-bottom: 1px solid #26262c;
    }
  }
</style>
