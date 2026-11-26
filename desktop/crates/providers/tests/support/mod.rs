//! A provider under test, and fake servers for it to talk to.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use chat_core::{ChatEvent, ChatMessage, ConnectionState};
use futures::{SinkExt, StreamExt};
use providers::{
    ChatProvider, ProviderCommand, ProviderConfig, ProviderEvent, ProviderHandle, ReconnectPolicy,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const PATIENCE: Duration = Duration::from_secs(10);

pub fn config() -> ProviderConfig {
    let mut config = ProviderConfig::from_env(|_| None, "tester");
    config.third_party_emotes = false;
    config
}

pub struct Client {
    handle: ProviderHandle,
    events: mpsc::UnboundedReceiver<ProviderEvent>,
}

impl Client {
    pub fn start(provider: impl ChatProvider) -> Self {
        let (sender, events) = mpsc::unbounded_channel();
        let policy = ReconnectPolicy {
            first: Duration::from_millis(50),
            max: Duration::from_millis(100),
        };
        Self {
            handle: providers::spawn(Box::new(provider), sender, policy),
            events,
        }
    }

    pub fn can_send(&self) -> bool {
        self.handle.can_send()
    }

    pub fn command(&self, command: ProviderCommand) {
        assert!(self.handle.command(command), "the provider has stopped");
    }

    pub fn say(&self, content: &str) {
        self.reply(content, None);
    }

    pub fn reply(&self, content: &str, reply_to: Option<&str>) {
        self.command(ProviderCommand::Send {
            content: content.into(),
            reply_to: reply_to.map(Into::into),
        });
    }

    pub async fn next(&mut self) -> ChatEvent {
        tokio::time::timeout(PATIENCE, self.events.recv())
            .await
            .expect("timed out waiting for an event")
            .expect("the provider stopped")
            .event
    }

    /// Skips events until `pick` takes one.
    pub async fn until<T>(&mut self, mut pick: impl FnMut(ChatEvent) -> Option<T>) -> T {
        loop {
            if let Some(found) = pick(self.next().await) {
                return found;
            }
        }
    }

    pub async fn state(&mut self, wanted: ConnectionState) {
        self.until(|event| (event == ChatEvent::ConnectionState(wanted.clone())).then_some(()))
            .await;
    }

    pub async fn connected(&mut self) {
        self.state(ConnectionState::Connected).await;
    }

    pub async fn error(&mut self) -> String {
        self.until(|event| match event {
            ChatEvent::ConnectionState(ConnectionState::Error(reason)) => Some(reason),
            _ => None,
        })
        .await
    }

    pub async fn system(&mut self) -> String {
        self.until(|event| match event {
            ChatEvent::System(text) => Some(text),
            _ => None,
        })
        .await
    }

    pub async fn message(&mut self, content: &str) -> ChatMessage {
        self.until(|event| match event {
            ChatEvent::Message(message) if message.content == content => Some(message),
            _ => None,
        })
        .await
    }
}

pub struct FakeSocketServer {
    listener: TcpListener,
}

impl FakeSocketServer {
    pub async fn start() -> Self {
        Self {
            listener: TcpListener::bind("127.0.0.1:0").await.unwrap(),
        }
    }

    pub fn url(&self) -> String {
        format!("ws://{}", self.listener.local_addr().unwrap())
    }

    pub async fn accept(&self) -> FakeSocket {
        let accepted = tokio::time::timeout(PATIENCE, self.listener.accept());
        let (stream, _) = accepted.await.expect("nobody connected").unwrap();
        FakeSocket(tokio_tungstenite::accept_async(stream).await.unwrap())
    }
}

pub struct FakeSocket(WebSocketStream<TcpStream>);

impl FakeSocket {
    pub async fn send(&mut self, text: &str) {
        self.0.send(Message::text(text)).await.unwrap();
    }

    pub async fn receive(&mut self) -> String {
        let frame = tokio::time::timeout(PATIENCE, self.0.next())
            .await
            .expect("timed out waiting for a frame")
            .expect("the client hung up")
            .unwrap();
        frame.into_text().unwrap().as_str().to_owned()
    }

    /// Skips frames until one that `wanted` accepts.
    pub async fn receive_where(&mut self, wanted: impl Fn(&str) -> bool) -> String {
        loop {
            let text = self.receive().await;
            if wanted(&text) {
                return text;
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    /// Method and path with its query.
    pub target: String,
    /// Lowercased header lines.
    pub headers: Vec<String>,
    pub body: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        let prefix = format!("{name}: ");
        self.headers
            .iter()
            .find_map(|line| line.strip_prefix(&prefix))
    }
}

/// Answers every request with what `answer` returns: a status and a JSON body.
pub struct FakeHttpServer {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl FakeHttpServer {
    pub async fn start(answer: impl Fn(&Request) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_request(&mut stream).await;
                let (status, body) = answer(&request);
                seen.lock().await.push(request);
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        Self { url, requests }
    }

    pub fn url(&self) -> String {
        self.url.clone()
    }

    pub async fn requests(&self) -> Vec<Request> {
        self.requests.lock().await.clone()
    }
}

async fn read_request(stream: &mut TcpStream) -> Request {
    let mut raw = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk).await.unwrap();
        raw.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&raw);
        let Some((head, body)) = text.split_once("\r\n\r\n") else {
            continue;
        };
        let mut lines = head.lines();
        let target = lines
            .next()
            .unwrap()
            .trim_end_matches(" HTTP/1.1")
            .to_owned();
        let request = Request {
            target,
            headers: lines.map(lowercase_name).collect(),
            body: body.to_owned(),
        };
        let expected = request
            .header("content-length")
            .map_or(0, |length| length.parse().unwrap());
        if body.len() >= expected {
            return request;
        }
    }
}

fn lowercase_name(line: &str) -> String {
    match line.split_once(": ") {
        Some((name, value)) => format!("{}: {value}", name.to_lowercase()),
        None => line.to_lowercase(),
    }
}
