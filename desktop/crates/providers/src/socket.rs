use std::sync::Once;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use crate::provider::SessionError;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A text websocket that reports silence as a dropped connection.
pub(crate) struct Socket {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    idle_limit: Duration,
    last_heard: Instant,
}

impl Socket {
    pub(crate) async fn connect(url: &str, idle_limit: Duration) -> Result<Self, SessionError> {
        install_tls();
        let connected = tokio::time::timeout(CONNECT_TIMEOUT, connect_async(url))
            .await
            .map_err(|_| dropped("timed out while connecting"))?;
        let (stream, _) = connected.map_err(dropped)?;
        Ok(Self {
            stream,
            idle_limit,
            last_heard: Instant::now(),
        })
    }

    pub(crate) async fn send(&mut self, text: impl Into<String>) -> Result<(), SessionError> {
        self.stream
            .send(Message::text(text.into()))
            .await
            .map_err(dropped)
    }

    /// The next text frame. Cancelling loses nothing.
    pub(crate) async fn next(&mut self) -> Result<String, SessionError> {
        loop {
            let deadline = self.last_heard + self.idle_limit;
            let frame = tokio::time::timeout_at(deadline, self.stream.next())
                .await
                .map_err(|_| dropped("the server went quiet"))?
                .ok_or_else(|| dropped("the server closed the connection"))?
                .map_err(dropped)?;
            self.last_heard = Instant::now();
            match frame {
                Message::Text(text) => return Ok(text.as_str().to_owned()),
                Message::Close(_) => return Err(dropped("the server closed the connection")),
                _ => {}
            }
        }
    }
}

pub(crate) fn dropped(reason: impl ToString) -> SessionError {
    SessionError::Dropped(reason.to_string())
}

/// GPUI's dependencies and ours each enable a different rustls backend, so
/// rustls cannot pick a default by itself.
fn install_tls() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        // Fails only if the process already chose one, which serves as well.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}
