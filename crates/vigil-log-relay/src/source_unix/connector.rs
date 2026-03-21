// ---------------------------------------------------------------------------
// Minimal Unix-socket connector for hyper
// ---------------------------------------------------------------------------

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use hyper::Uri;
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tower::Service;

#[derive(Clone)]
pub(super) struct UnixConnector(pub(super) Arc<PathBuf>);

impl Service<Uri> for UnixConnector {
    type Response = TokioIo<UnixStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let path = Arc::clone(&self.0);
        Box::pin(async move {
            let stream = UnixStream::connect(path.as_path()).await?;
            Ok(TokioIo::new(stream))
        })
    }
}
