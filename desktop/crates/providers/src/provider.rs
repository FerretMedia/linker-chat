use std::time::Duration;

use chat_core::{ChannelKey, ChatEvent, ConnectionState, ModAction};
use futures::future::BoxFuture;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderEvent {
    pub channel: ChannelKey,
    pub event: ChatEvent,
}

pub type EventSender = mpsc::UnboundedSender<ProviderEvent>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCommand {
    Send {
        content: String,
        reply_to: Option<String>,
    },
    Moderate(ModAction),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// The connection went away and is worth retrying.
    #[error("{0}")]
    Dropped(String),
    /// Retrying would fail the same way.
    #[error("{0}")]
    Fatal(String),
}

pub trait ChatProvider: Send + 'static {
    fn channel(&self) -> &ChannelKey;

    /// Whether the credentials allow sending. `false` means read-only.
    fn can_send(&self) -> bool;

    /// Runs one connection. `Ok` means the owner hung up; an error says whether
    /// [`spawn`] should dial again.
    fn run_session<'a>(
        &'a mut self,
        io: &'a mut SessionIo,
    ) -> BoxFuture<'a, Result<(), SessionError>>;
}

/// A running session's link to the app.
pub struct SessionIo {
    channel: ChannelKey,
    events: EventSender,
    commands: mpsc::UnboundedReceiver<ProviderCommand>,
    connected: bool,
}

impl SessionIo {
    pub fn emit(&self, event: ChatEvent) {
        // The receiver only closes when the app is shutting down.
        let _ = self.events.send(ProviderEvent {
            channel: self.channel.clone(),
            event,
        });
    }

    pub fn emit_all(&self, events: impl IntoIterator<Item = ChatEvent>) {
        events.into_iter().for_each(|event| self.emit(event));
    }

    pub fn system(&self, text: impl Into<String>) {
        self.emit(ChatEvent::System(text.into()));
    }

    pub fn connected(&mut self) {
        if !std::mem::replace(&mut self.connected, true) {
            self.emit(ChatEvent::ConnectionState(ConnectionState::Connected));
        }
    }

    /// The next request from the app, or `None` once it hangs up.
    pub async fn command(&mut self) -> Option<ProviderCommand> {
        self.commands.recv().await
    }

    /// Loads emotes without holding up the session.
    pub fn load_emotes(&self, fetch: impl Future<Output = Vec<chat_core::Emote>> + Send + 'static) {
        let events = self.events.clone();
        let channel = self.channel.clone();
        tokio::spawn(async move {
            let event = ChatEvent::Emotes(fetch.await);
            let _ = events.send(ProviderEvent { channel, event });
        });
    }

    /// Waits out a reconnect delay. `false` means the app hung up.
    async fn wait(&mut self, delay: Duration) -> bool {
        let deadline = tokio::time::sleep(delay);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                () = &mut deadline => return true,
                command = self.commands.recv() => match command {
                    Some(_) => self.system("Not connected, so nothing was sent."),
                    None => return false,
                },
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReconnectPolicy {
    pub first: Duration,
    pub max: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(1),
            max: Duration::from_secs(30),
        }
    }
}

impl ReconnectPolicy {
    /// Doubles from `first` up to `max`. Attempts count from 1.
    fn delay(&self, attempt: u32) -> Duration {
        let doublings = attempt.saturating_sub(1).min(16);
        self.max.min(self.first * 2_u32.pow(doublings))
    }
}

/// The app's end of a running provider. Dropping it disconnects.
pub struct ProviderHandle {
    commands: mpsc::UnboundedSender<ProviderCommand>,
    can_send: bool,
    task: JoinHandle<()>,
}

impl ProviderHandle {
    pub fn can_send(&self) -> bool {
        self.can_send
    }

    /// `false` if the provider has stopped for good.
    pub fn command(&self, command: ProviderCommand) -> bool {
        self.commands.send(command).is_ok()
    }
}

impl Drop for ProviderHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Keeps `provider` connected until its handle drops or it fails for good.
/// Must be called inside a tokio runtime.
pub fn spawn(
    provider: Box<dyn ChatProvider>,
    events: EventSender,
    policy: ReconnectPolicy,
) -> ProviderHandle {
    let (commands, receiver) = mpsc::unbounded_channel();
    let can_send = provider.can_send();
    let io = SessionIo {
        channel: provider.channel().clone(),
        events,
        commands: receiver,
        connected: false,
    };
    ProviderHandle {
        commands,
        can_send,
        task: tokio::spawn(supervise(provider, io, policy)),
    }
}

async fn supervise(
    mut provider: Box<dyn ChatProvider>,
    mut io: SessionIo,
    policy: ReconnectPolicy,
) {
    let state = |io: &SessionIo, state| io.emit(ChatEvent::ConnectionState(state));
    let mut attempt = 0;
    state(&io, ConnectionState::Connecting);
    loop {
        io.connected = false;
        match provider.run_session(&mut io).await {
            Ok(()) => return state(&io, ConnectionState::Disconnected),
            Err(SessionError::Fatal(reason)) => return state(&io, ConnectionState::Error(reason)),
            Err(SessionError::Dropped(reason)) => {
                if io.connected {
                    attempt = 0;
                }
                attempt += 1;
                if attempt == 1 {
                    let what = if io.connected {
                        "Connection lost"
                    } else {
                        "Could not connect"
                    };
                    io.system(format!("{what}, trying again. {reason}"));
                }
                state(&io, ConnectionState::Reconnecting { attempt });
                if !io.wait(policy.delay(attempt)).await {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    type Outcome = (bool, Result<(), SessionError>);

    struct Scripted {
        channel: ChannelKey,
        sessions: VecDeque<Outcome>,
    }

    impl ChatProvider for Scripted {
        fn channel(&self) -> &ChannelKey {
            &self.channel
        }

        fn can_send(&self) -> bool {
            true
        }

        fn run_session<'a>(
            &'a mut self,
            io: &'a mut SessionIo,
        ) -> BoxFuture<'a, Result<(), SessionError>> {
            let (connects, outcome) = self.sessions.pop_front().expect("no more sessions");
            Box::pin(async move {
                if connects {
                    io.connected();
                }
                outcome
            })
        }
    }

    async fn states(sessions: Vec<Outcome>) -> Vec<ConnectionState> {
        let provider = Scripted {
            channel: "native:main".parse().unwrap(),
            sessions: sessions.into(),
        };
        let (events, mut received) = mpsc::unbounded_channel();
        let policy = ReconnectPolicy {
            first: Duration::from_millis(1),
            max: Duration::from_millis(2),
        };
        let _handle = spawn(Box::new(provider), events, policy);
        let mut states = Vec::new();
        while let Some(event) = received.recv().await {
            if let ChatEvent::ConnectionState(state) = event.event {
                states.push(state);
            }
        }
        states
    }

    fn dropped() -> Result<(), SessionError> {
        Err(SessionError::Dropped("gone".into()))
    }

    #[tokio::test]
    async fn redials_until_a_fatal_error_and_restarts_the_count_after_success() {
        use ConnectionState::*;
        let fatal = Err(SessionError::Fatal("bad token".into()));
        assert_eq!(
            states(vec![
                (false, dropped()),
                (false, dropped()),
                (true, dropped()),
                (false, fatal)
            ])
            .await,
            [
                Connecting,
                Reconnecting { attempt: 1 },
                Reconnecting { attempt: 2 },
                Connected,
                Reconnecting { attempt: 1 },
                Error("bad token".into()),
            ]
        );
    }

    #[tokio::test]
    async fn a_clean_exit_ends_disconnected() {
        use ConnectionState::*;
        assert_eq!(
            states(vec![(true, Ok(()))]).await,
            [Connecting, Connected, Disconnected]
        );
    }

    #[test]
    fn delays_double_up_to_the_cap() {
        let policy = ReconnectPolicy::default();
        let secs: Vec<u64> = (1..=7).map(|n| policy.delay(n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30]);
        assert_eq!(policy.delay(u32::MAX).as_secs(), 30);
    }
}
