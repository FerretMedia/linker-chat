use std::collections::HashMap;
use std::sync::Arc;

use crate::message::{Emote, InlineEmote, MessageFragment};
use crate::platform::ChannelKey;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EmoteScope {
    Global,
    Channel(ChannelKey),
}

/// Emotes by code. A channel's own emotes shadow global ones.
#[derive(Debug, Default)]
pub struct EmoteRegistry {
    sets: HashMap<EmoteScope, HashMap<String, Arc<Emote>>>,
}

impl EmoteRegistry {
    pub fn install(&mut self, scope: EmoteScope, emotes: impl IntoIterator<Item = Emote>) {
        self.sets.entry(scope).or_default().extend(
            emotes
                .into_iter()
                .map(|emote| (emote.code.clone(), Arc::new(emote))),
        );
    }

    /// Finds `code` as typed in `channel`.
    pub fn lookup(&self, channel: &ChannelKey, code: &str) -> Option<Arc<Emote>> {
        [EmoteScope::Channel(channel.clone()), EmoteScope::Global]
            .iter()
            .find_map(|scope| self.sets.get(scope)?.get(code))
            .cloned()
    }
}

/// Splits `content` into what a renderer draws. Joining the fragments' text
/// gives `content` back, whitespace included.
pub fn tokenize_message(
    content: &str,
    inline_emotes: &[InlineEmote],
    channel: &ChannelKey,
    registry: &EmoteRegistry,
) -> Vec<MessageFragment> {
    let mut fragments = Fragments::default();
    let mut inline: Vec<&InlineEmote> = inline_emotes.iter().collect();
    inline.sort_by_key(|emote| emote.range.start);

    let mut position = 0;
    for placed in inline {
        // Ranges come from the network, so skip any that overlap or split a character.
        let Some(before) = content.get(position..placed.range.start) else {
            continue;
        };
        if content.get(placed.range.clone()).is_none() {
            continue;
        }
        fragments.push_words(before, channel, registry);
        fragments.push(MessageFragment::Emote(Arc::new(placed.emote.clone())));
        position = placed.range.end;
    }
    fragments.push_words(&content[position..], channel, registry);
    fragments.finish()
}

#[derive(Default)]
struct Fragments {
    done: Vec<MessageFragment>,
    text: String,
}

impl Fragments {
    fn push(&mut self, fragment: MessageFragment) {
        self.flush_text();
        self.done.push(fragment);
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            let text = std::mem::take(&mut self.text);
            self.done.push(MessageFragment::Text(text));
        }
    }

    fn finish(mut self) -> Vec<MessageFragment> {
        self.flush_text();
        self.done
    }

    fn push_words(&mut self, text: &str, channel: &ChannelKey, registry: &EmoteRegistry) {
        for piece in text.split_inclusive(char::is_whitespace) {
            let word = piece.trim_end();
            let space = &piece[word.len()..];
            self.push_word(word, channel, registry);
            self.text.push_str(space);
        }
    }

    fn push_word(&mut self, word: &str, channel: &ChannelKey, registry: &EmoteRegistry) {
        if let Some(emote) = registry.lookup(channel, word) {
            self.push(MessageFragment::Emote(emote));
            return;
        }
        let (fragment, rest) = if let Some(split) = split_mention(word) {
            split
        } else if let Some(split) = split_link(word) {
            split
        } else {
            self.text.push_str(word);
            return;
        };
        self.push(fragment);
        self.text.push_str(rest);
    }
}

fn split_mention(word: &str) -> Option<(MessageFragment, &str)> {
    let name = word.strip_prefix('@')?;
    let end = name
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(name.len());
    (end > 0).then(|| {
        (
            MessageFragment::Mention(name[..end].to_owned()),
            &name[end..],
        )
    })
}

fn split_link(word: &str) -> Option<(MessageFragment, &str)> {
    let host = word
        .strip_prefix("https://")
        .or_else(|| word.strip_prefix("http://"))?;
    let end = word
        .trim_end_matches(['.', ',', '!', '?', ';', ':', ')', '"', '\''])
        .len();
    (!host.is_empty() && end > word.len() - host.len())
        .then(|| (MessageFragment::Link(word[..end].to_owned()), &word[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::EmoteProvider;

    fn emote(code: &str, provider: EmoteProvider) -> Emote {
        Emote {
            code: code.into(),
            url: format!("https://cdn.example/{code}.webp"),
            animated: false,
            provider,
        }
    }

    fn channel() -> ChannelKey {
        "twitch:someone".parse().unwrap()
    }

    fn registry() -> EmoteRegistry {
        let mut registry = EmoteRegistry::default();
        registry.install(
            EmoteScope::Global,
            [emote("catJAM", EmoteProvider::SevenTv)],
        );
        registry
    }

    fn tokenize(content: &str) -> Vec<MessageFragment> {
        tokenize_message(content, &[], &channel(), &registry())
    }

    fn text(s: &str) -> MessageFragment {
        MessageFragment::Text(s.into())
    }

    fn named(code: &str) -> MessageFragment {
        MessageFragment::Emote(Arc::new(emote(code, EmoteProvider::SevenTv)))
    }

    #[test]
    fn splits_text_emotes_mentions_and_links() {
        assert_eq!(
            tokenize("hey @Ferret_42, catJAM see https://example.com/a?b=1."),
            [
                text("hey "),
                MessageFragment::Mention("Ferret_42".into()),
                text(", "),
                named("catJAM"),
                text(" see "),
                MessageFragment::Link("https://example.com/a?b=1".into()),
                text("."),
            ]
        );
    }

    #[test]
    fn emote_codes_match_whole_words_and_case() {
        assert_eq!(tokenize("catJAMMER catjam"), [text("catJAMMER catjam")]);
        assert_eq!(
            tokenize("catJAM  catJAM"),
            [named("catJAM"), text("  "), named("catJAM")]
        );
    }

    #[test]
    fn bare_markers_stay_text() {
        assert_eq!(
            tokenize("@ @! https:// http://..."),
            [text("@ @! https:// http://...")]
        );
        assert_eq!(tokenize(""), []);
    }

    #[test]
    fn channel_emotes_shadow_global_ones_only_in_that_channel() {
        let mut registry = registry();
        registry.install(
            EmoteScope::Channel(channel()),
            [emote("catJAM", EmoteProvider::Bttv)],
        );
        let elsewhere: ChannelKey = "twitch:other".parse().unwrap();
        let provider = |key: &ChannelKey| registry.lookup(key, "catJAM").unwrap().provider;
        assert_eq!(provider(&channel()), EmoteProvider::Bttv);
        assert_eq!(provider(&elsewhere), EmoteProvider::SevenTv);
    }

    #[test]
    fn inline_emotes_land_on_their_byte_ranges() {
        let placed = |range, code: &str| InlineEmote {
            range,
            emote: emote(code, EmoteProvider::Twitch),
        };
        let content = "é Kappa ok Kappa";
        let fragments = tokenize_message(
            content,
            &[placed(12..17, "Kappa"), placed(3..8, "Kappa")],
            &channel(),
            &EmoteRegistry::default(),
        );
        let kappa = MessageFragment::Emote(Arc::new(emote("Kappa", EmoteProvider::Twitch)));
        assert_eq!(fragments, [text("é "), kappa.clone(), text(" ok "), kappa]);
    }

    #[test]
    fn hostile_inline_ranges_are_ignored() {
        let placed = |range| InlineEmote {
            range,
            emote: emote("x", EmoteProvider::Twitch),
        };
        let fragments = tokenize_message(
            "é ok",
            &[placed(1..2), placed(0..99), placed(50..60)],
            &channel(),
            &EmoteRegistry::default(),
        );
        assert_eq!(fragments, [text("é ok")]);
    }
}
