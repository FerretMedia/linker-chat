use std::time::Duration;

/// How to sign in to the Linker-chat server.
#[derive(Clone)]
pub enum NativeAuth {
    /// Read-only.
    Lurker,
    Guest {
        nickname: String,
    },
    /// Personal or admin API token.
    Token {
        token: String,
        display_name: Option<String>,
    },
    /// Clerk session token.
    Jwt(String),
}

#[derive(Clone)]
pub struct NativeConfig {
    pub server_url: String,
    pub auth: NativeAuth,
}

#[derive(Clone)]
pub enum TwitchAuth {
    Anonymous,
    OAuth { login: String, token: String },
}

/// Reading needs either credential. Sending needs the OAuth token.
#[derive(Clone, Default)]
pub struct YouTubeCredentials {
    pub api_key: Option<String>,
    pub access_token: Option<String>,
}

#[derive(Clone)]
pub struct Endpoints {
    pub twitch_irc: String,
    pub kick_site: String,
    /// Where Kick says which Pusher app its chat is on.
    pub kick_realtime: String,
    /// `{cluster}` is filled in from what Kick answers.
    pub kick_pusher: String,
    pub kick_api: String,
    pub youtube_api: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            twitch_irc: "wss://irc-ws.chat.twitch.tv:443".into(),
            kick_site: "https://kick.com".into(),
            kick_realtime: "https://web.kick.com".into(),
            kick_pusher: "wss://ws-{cluster}.pusher.com".into(),
            kick_api: "https://api.kick.com".into(),
            youtube_api: "https://www.googleapis.com".into(),
        }
    }
}

#[derive(Clone)]
pub struct ProviderConfig {
    pub native: NativeConfig,
    pub twitch: TwitchAuth,
    pub kick_token: Option<String>,
    pub youtube: YouTubeCredentials,
    pub third_party_emotes: bool,
    pub endpoints: Endpoints,
    pub http: reqwest::Client,
}

impl ProviderConfig {
    /// Reads the `LINKER_CHAT_*` variables through `lookup`. `guest_nickname`
    /// signs in to the native server when no token or nickname is set.
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>, guest_nickname: &str) -> Self {
        let var = |name: &str| {
            lookup(&format!("LINKER_CHAT_{name}"))
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let auth = if let Some(token) = var("TOKEN") {
            NativeAuth::Token {
                token,
                display_name: var("DISPLAY_NAME"),
            }
        } else if let Some(jwt) = var("JWT") {
            NativeAuth::Jwt(jwt)
        } else {
            NativeAuth::Guest {
                nickname: var("NICKNAME").unwrap_or_else(|| guest_nickname.to_owned()),
            }
        };
        let twitch = match (var("TWITCH_LOGIN"), var("TWITCH_TOKEN")) {
            (Some(login), Some(token)) => TwitchAuth::OAuth {
                login: login.to_lowercase(),
                token: token.trim_start_matches("oauth:").to_owned(),
            },
            _ => TwitchAuth::Anonymous,
        };
        Self {
            native: NativeConfig {
                server_url: var("SERVER").unwrap_or_else(|| "ws://127.0.0.1:8080/ws".into()),
                auth,
            },
            twitch,
            kick_token: var("KICK_TOKEN"),
            youtube: YouTubeCredentials {
                api_key: var("YOUTUBE_API_KEY"),
                access_token: var("YOUTUBE_TOKEN"),
            },
            third_party_emotes: true,
            endpoints: Endpoints::default(),
            http: http_client(),
        }
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("linker-chat-desktop/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .build()
        .expect("the TLS backend is compiled in")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(vars: &[(&str, &str)]) -> ProviderConfig {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(name, value)| (format!("LINKER_CHAT_{name}"), value.to_string()))
            .collect();
        ProviderConfig::from_env(
            |name| {
                vars.iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, value)| value.clone())
            },
            "desktop-1",
        )
    }

    #[test]
    fn an_empty_environment_is_a_local_guest_and_anonymous_twitch() {
        let config = config(&[("TOKEN", "  ")]);
        assert_eq!(config.native.server_url, "ws://127.0.0.1:8080/ws");
        assert!(matches!(
            config.native.auth,
            NativeAuth::Guest { nickname } if nickname == "desktop-1"
        ));
        assert!(matches!(config.twitch, TwitchAuth::Anonymous));
    }

    #[test]
    fn a_token_wins_over_a_nickname() {
        let config = config(&[("TOKEN", "secret"), ("NICKNAME", "me")]);
        assert!(matches!(
            config.native.auth,
            NativeAuth::Token { token, .. } if token == "secret"
        ));
    }

    #[test]
    fn twitch_needs_both_login_and_token() {
        assert!(matches!(
            config(&[("TWITCH_TOKEN", "oauth:abc")]).twitch,
            TwitchAuth::Anonymous
        ));
        let full = config(&[("TWITCH_TOKEN", "oauth:abc"), ("TWITCH_LOGIN", "Me")]);
        assert!(matches!(
            full.twitch,
            TwitchAuth::OAuth { login, token } if login == "me" && token == "abc"
        ));
    }
}
