import type { Emote, EmoteOptions, MessageFragment } from './types.js';

const URL_REGEX = /^https?:\/\/[^\s/$.?#].[^\s]*$/i;
const MENTION_REGEX = /^@[a-zA-Z0-9_-]{2,32}[,.:;!?]?$/;

/**
 * Fetches optional third-party emote packs (7TV / BTTV) for rich streaming chat rendering.
 * Fails gracefully if offline or blocked by CORS/network policies.
 */
export async function loadThirdPartyEmotes(opts: EmoteOptions = {}): Promise<Emote[]> {
  const results: Emote[] = [];

  const tasks: Promise<void>[] = [];

  if (opts.sevenTvGlobal) {
    tasks.push(
      fetchSevenTvSet('global')
        .then((list) => {
          results.push(...list);
        })
        .catch(() => {})
    );
  }
  if (opts.sevenTvEmoteSetId) {
    tasks.push(
      fetchSevenTvSet(opts.sevenTvEmoteSetId)
        .then((list) => {
          results.push(...list);
        })
        .catch(() => {})
    );
  }
  if (opts.bttvGlobal) {
    tasks.push(
      fetchBttvGlobal()
        .then((list) => {
          results.push(...list);
        })
        .catch(() => {})
    );
  }

  await Promise.all(tasks);
  return results;
}

async function fetchSevenTvSet(setId: string): Promise<Emote[]> {
  const res = await fetch(`https://7tv.io/v3/emote-sets/${encodeURIComponent(setId)}`);
  if (!res.ok) return [];
  const json = (await res.json()) as {
    emotes?: Array<{
      id: string;
      name: string;
      data?: { animated?: boolean; host?: { url?: string } };
    }>;
  };
  return (json.emotes ?? []).map((e) => {
    const base = e.data?.host?.url ? `https:${e.data.host.url}` : `https://cdn.7tv.app/emote/${e.id}`;
    return {
      code: e.name,
      url: `${base}/2x.webp`,
      animated: Boolean(e.data?.animated),
      provider: '7tv' as const,
    };
  });
}

async function fetchBttvGlobal(): Promise<Emote[]> {
  const res = await fetch('https://api.betterttv.net/3/cached/emotes/global');
  if (!res.ok) return [];
  const json = (await res.json()) as Array<{ id: string; code: string; animated?: boolean }>;
  return json.map((e) => ({
    code: e.code,
    url: `https://cdn.betterttv.net/emote/${e.id}/2x.webp`,
    animated: Boolean(e.animated),
    provider: 'bttv' as const,
  }));
}

/**
 * Splits a chat message into text, emote, mention, and link fragments for fast rendering.
 */
export function tokenizeMessage(content: string, emotes: Map<string, Emote>): MessageFragment[] {
  const parts = content.split(/(\s+)/);
  const fragments: MessageFragment[] = [];

  const pushText = (text: string) => {
    const prev = fragments[fragments.length - 1];
    if (prev && prev.kind === 'text') {
      prev.text += text;
    } else {
      fragments.push({ kind: 'text', text });
    }
  };

  for (const part of parts) {
    if (!part) continue;
    if (/^\s+$/.test(part)) {
      pushText(part);
      continue;
    }
    const emote = emotes.get(part);
    if (emote) {
      fragments.push({ kind: 'emote', emote });
      continue;
    }
    if (URL_REGEX.test(part)) {
      fragments.push({ kind: 'link', url: part });
      continue;
    }
    if (MENTION_REGEX.test(part)) {
      const clean = part.slice(1).replace(/[,.:;!?]+$/, '');
      fragments.push({ kind: 'mention', username: clean, raw: part });
      continue;
    }
    pushText(part);
  }
  return fragments;
}
