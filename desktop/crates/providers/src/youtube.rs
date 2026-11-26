mod api;

use std::time::Duration;

use chat_core::{ChannelKey, ChatEvent, RoomState};
use futures::future::BoxFuture;
use reqwest::{RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::time::Instant;

use crate::config::ProviderConfig;
use crate::provider::{ChatProvider, ProviderCommand, SessionError, SessionIo};
use crate::socket::dropped;
use crate::third_party;
use api::{MessagePage, VideoList, error_reason};

/// Each poll costs API quota, so never poll faster than this.
const MIN_POLL: Duration = Duration::from_secs(2);

/// YouTube Live Chat over the Data API, resuming from `nextPageToken`.
pub struct YouTubeProvider {
    channel: ChannelKey,
    config: ProviderConfig,
    min_poll: Duration,
    live_chat_id: Option<String>,
    page_token: Option<String>,
}

impl YouTubeProvider {
    pub fn new(channel: ChannelKey, config: &ProviderConfig) -> Self {
        Self {
            channel,
            config: config.clone(),
            min_poll: MIN_POLL,
            live_chat_id: None,
            page_token: None,
        }
    }

    /// Overrides the shortest time between polls.
    pub fn with_min_poll(mut self, min_poll: Duration) -> Self {
        self.min_poll = min_poll;
        self
    }

    async fn session(&mut self, io: &mut SessionIo) -> Result<(), SessionError> {
        let credentials = &self.config.youtube;
        if credentials.api_key.is_none() && credentials.access_token.is_none() {
            return Err(SessionError::Fatal(
                "Set LINKER_CHAT_YOUTUBE_API_KEY to read YouTube chat.".into(),
            ));
        }
        let live_chat_id = match self.live_chat_id.clone() {
            Some(id) => id,
            None => self.find_live_chat(io).await?,
        };
        self.live_chat_id = Some(live_chat_id.clone());

        loop {
            let page = self.poll(&live_chat_id).await?;
            io.connected();
            let interval = Duration::from_millis(page.polling_interval_millis.unwrap_or_default());
            self.page_token = page.next_page_token;
            io.emit_all(page.items.into_iter().filter_map(|item| item.into_event()));
            if page.offline_at.is_some() || self.page_token.is_none() {
                return Err(SessionError::Fatal("The live chat ended.".into()));
            }

            let next_poll = Instant::now() + interval.max(self.min_poll);
            loop {
                tokio::select! {
                    () = tokio::time::sleep_until(next_poll) => break,
                    command = io.command() => match command {
                        None => return Ok(()),
                        Some(ProviderCommand::Moderate(_)) => {
                            io.system("Moderating YouTube from here is not supported.");
                        }
                        Some(ProviderCommand::Send { content, .. }) => {
                            if let Err(reason) = self.send(&live_chat_id, &content).await {
                                io.system(format!("Not sent. {reason}"));
                            }
                        }
                    },
                }
            }
        }
    }

    async fn find_live_chat(&self, io: &SessionIo) -> Result<String, SessionError> {
        let request = self.get("/youtube/v3/videos").query(&[
            ("part", "snippet,liveStreamingDetails"),
            ("id", self.channel.channel()),
        ]);
        let videos: VideoList = fetch(request).await.map_err(ApiError::into_session)?;
        let video = videos.items.into_iter().next().ok_or_else(|| {
            SessionError::Fatal(format!("There is no video {}.", self.channel.channel()))
        })?;
        let details = video.live_streaming_details;
        io.emit(ChatEvent::RoomState(RoomState {
            title: Some(video.snippet.title),
            viewer_count: details.as_ref().and_then(|details| details.viewers()),
            ..RoomState::default()
        }));
        if self.config.third_party_emotes {
            let http = self.config.http.clone();
            let owner = video.snippet.channel_id;
            io.load_emotes(async move { third_party::fetch_youtube_channel(&http, &owner).await });
        }
        details
            .and_then(|details| details.active_live_chat_id)
            .ok_or_else(|| SessionError::Fatal("That video has no live chat right now.".into()))
    }

    async fn poll(&mut self, live_chat_id: &str) -> Result<MessagePage, SessionError> {
        let mut request = self.get("/youtube/v3/liveChat/messages").query(&[
            ("liveChatId", live_chat_id),
            ("part", "id,snippet,authorDetails"),
            ("maxResults", "200"),
        ]);
        if let Some(token) = &self.page_token {
            request = request.query(&[("pageToken", token)]);
        }
        match fetch(request).await {
            Ok(page) => Ok(page),
            Err(error) => {
                // An expired token would fail every retry. Starting over only
                // re-reads messages the store already has.
                if error.reason.as_deref() == Some("pageTokenInvalid") {
                    self.page_token = None;
                }
                Err(error.into_session())
            }
        }
    }

    async fn send(&self, live_chat_id: &str, content: &str) -> Result<(), String> {
        let token = self
            .config
            .youtube
            .access_token
            .as_ref()
            .ok_or("Sign in to YouTube to chat here.")?;
        let request = self
            .config
            .http
            .post(self.url("/youtube/v3/liveChat/messages"))
            .query(&[("part", "snippet")])
            .bearer_auth(token)
            .json(&json!({
                "snippet": {
                    "liveChatId": live_chat_id,
                    "type": "textMessageEvent",
                    "textMessageDetails": { "messageText": content },
                }
            }));
        fetch::<serde_json::Value>(request)
            .await
            .map(drop)
            .map_err(|error| error.message())
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.endpoints.youtube_api)
    }

    fn get(&self, path: &str) -> RequestBuilder {
        let request = self.config.http.get(self.url(path));
        let credentials = &self.config.youtube;
        match (&credentials.access_token, &credentials.api_key) {
            (Some(token), _) => request.bearer_auth(token),
            (None, Some(key)) => request.header("X-Goog-Api-Key", key),
            (None, None) => request,
        }
    }
}

impl ChatProvider for YouTubeProvider {
    fn channel(&self) -> &ChannelKey {
        &self.channel
    }

    fn can_send(&self) -> bool {
        self.config.youtube.access_token.is_some()
    }

    fn run_session<'a>(
        &'a mut self,
        io: &'a mut SessionIo,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(self.session(io))
    }
}

struct ApiError {
    status: Option<StatusCode>,
    reason: Option<String>,
    detail: String,
}

impl ApiError {
    fn message(&self) -> String {
        match (&self.reason, self.status) {
            (Some(reason), _) => format!("YouTube answered {reason}."),
            (None, Some(status)) => format!("YouTube answered {status}."),
            (None, None) => self.detail.clone(),
        }
    }

    fn into_session(self) -> SessionError {
        let message = self.message();
        let retryable = match self.status {
            None => true,
            Some(status) if status.is_server_error() => true,
            Some(StatusCode::TOO_MANY_REQUESTS) => true,
            Some(_) => matches!(
                self.reason.as_deref(),
                Some("pageTokenInvalid" | "rateLimitExceeded")
            ),
        };
        if retryable {
            dropped(message)
        } else {
            SessionError::Fatal(message)
        }
    }
}

async fn fetch<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, ApiError> {
    let transport = |error: reqwest::Error| ApiError {
        status: None,
        reason: None,
        detail: error.without_url().to_string(),
    };
    let response = request.send().await.map_err(transport)?;
    let status = response.status();
    if status.is_success() {
        return response.json().await.map_err(transport);
    }
    let body = response.text().await.unwrap_or_default();
    Err(ApiError {
        status: Some(status),
        reason: error_reason(&body),
        detail: body,
    })
}
