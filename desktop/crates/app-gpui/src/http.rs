use futures::FutureExt;
use futures::future::BoxFuture;
use gpui::http_client::{self, AsyncBody, HttpClient, Request, Response, Url};

/// Lets GPUI load emote images with the app's own HTTP client.
pub struct ImageHttp {
    client: reqwest::Client,
    runtime: tokio::runtime::Handle,
}

impl ImageHttp {
    pub fn new(client: reqwest::Client, runtime: tokio::runtime::Handle) -> Self {
        Self { client, runtime }
    }
}

impl HttpClient for ImageHttp {
    fn type_name(&self) -> &'static str {
        "ImageHttp"
    }

    fn user_agent(&self) -> Option<&http_client::http::HeaderValue> {
        None
    }

    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let url = request.uri().to_string();
        let client = self.client.clone();
        let fetch = self.runtime.spawn(async move {
            let response = client.get(url).send().await?;
            let status = response.status();
            let body = response.bytes().await?;
            Ok(Response::builder()
                .status(status)
                .body(AsyncBody::from(body))?)
        });
        async move { fetch.await? }.boxed()
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }
}
