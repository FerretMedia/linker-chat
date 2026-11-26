//! IRCv3 lines as Twitch sends them.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrcMessage<'a> {
    pub tags: HashMap<&'a str, String>,
    /// The sender's nick, when the prefix names a user.
    pub nick: Option<&'a str>,
    pub command: &'a str,
    pub params: Vec<&'a str>,
}

impl IrcMessage<'_> {
    /// A tag's value, treating an empty one as absent.
    pub fn tag(&self, name: &str) -> Option<&str> {
        self.tags
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    pub fn param(&self, index: usize) -> Option<&str> {
        self.params.get(index).copied()
    }
}

/// `None` for a line with no command.
pub fn parse_line(line: &str) -> Option<IrcMessage<'_>> {
    let mut rest = line.trim_end_matches(['\r', '\n']);

    let mut tags = HashMap::new();
    if let Some(tagged) = rest.strip_prefix('@') {
        let (raw, after) = tagged.split_once(' ')?;
        for pair in raw.split(';') {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            tags.insert(name, unescape(value));
        }
        rest = after.trim_start();
    }

    let mut nick = None;
    if let Some(prefixed) = rest.strip_prefix(':') {
        let (prefix, after) = prefixed.split_once(' ')?;
        nick = prefix.split_once('!').map(|(nick, _)| nick);
        rest = after.trim_start();
    }

    let (head, trailing) = match rest.split_once(" :") {
        Some((head, trailing)) => (head, Some(trailing)),
        None => (rest, None),
    };
    let mut words = head.split_ascii_whitespace();
    let command = words.next()?;
    Some(IrcMessage {
        tags,
        nick,
        command,
        params: words.chain(trailing).collect(),
    })
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(':') => out.push(';'),
                Some('s') => out.push(' '),
                Some('r') => out.push('\r'),
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            },
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tags_prefix_command_and_trailing() {
        let line = "@badge-info=;color=#1E90FF;display-name=Some\\sOne;flags= \
                    :someone!someone@someone.tmi.twitch.tv PRIVMSG #chan :hello :) there\r\n";
        let message = parse_line(line).unwrap();
        assert_eq!(message.tag("color"), Some("#1E90FF"));
        assert_eq!(message.tag("display-name"), Some("Some One"));
        assert_eq!(message.tag("badge-info"), None);
        assert_eq!(message.nick, Some("someone"));
        assert_eq!(message.command, "PRIVMSG");
        assert_eq!(message.params, ["#chan", "hello :) there"]);
    }

    #[test]
    fn parses_lines_without_tags_or_prefix() {
        let ping = parse_line("PING :tmi.twitch.tv").unwrap();
        assert_eq!(ping.command, "PING");
        assert_eq!(ping.params, ["tmi.twitch.tv"]);
        assert_eq!(ping.nick, None);

        let welcome = parse_line(":tmi.twitch.tv 001 someone :Welcome, GLHF!").unwrap();
        assert_eq!(welcome.command, "001");
        assert_eq!(welcome.nick, None);
        assert_eq!(welcome.params, ["someone", "Welcome, GLHF!"]);
    }

    #[test]
    fn unescapes_every_tag_sequence() {
        assert_eq!(unescape(r"a\:b\sc\\d\re\nf\xg\"), "a;b c\\d\re\nfxg");
    }

    #[test]
    fn rejects_lines_with_no_command() {
        for line in ["", "\r\n", "@tags=only", ":prefix.only", "@a=b :prefix"] {
            assert_eq!(parse_line(line), None, "{line:?}");
        }
    }
}
