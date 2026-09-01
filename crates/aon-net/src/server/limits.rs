use std::future::Future;
use std::io;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::serve::Listener;
use axum_server::accept::Accept;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub(super) struct ConnectionGate {
    permits: Arc<Semaphore>,
}

impl ConnectionGate {
    pub(super) fn new(limit: NonZeroUsize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(limit.get())),
        }
    }

    pub(super) async fn accept(&self, listener: &TcpListener) -> io::Result<AcceptedTcpConnection> {
        let (stream, peer) = listener.accept().await?;
        let permit = self.acquire().await;
        Ok(AcceptedTcpConnection {
            stream,
            peer,
            _permit: permit,
        })
    }

    async fn acquire(&self) -> OwnedSemaphorePermit {
        match Arc::clone(&self.permits).acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => std::future::pending().await,
        }
    }

    fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.permits).try_acquire_owned().ok()
    }
}

pub(super) struct AcceptedTcpConnection {
    stream: TcpStream,
    peer: std::net::SocketAddr,
    _permit: OwnedSemaphorePermit,
}

impl AcceptedTcpConnection {
    pub(super) fn into_parts(self) -> (TcpStream, std::net::SocketAddr, OwnedSemaphorePermit) {
        (self.stream, self.peer, self._permit)
    }
}

pub(super) struct LimitedListener {
    listener: TcpListener,
    connections: ConnectionGate,
}

impl LimitedListener {
    pub(super) fn new(listener: TcpListener, connections: ConnectionGate) -> Self {
        Self {
            listener,
            connections,
        }
    }
}

impl Listener for LimitedListener {
    type Io = PermitStream<TcpStream>;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (stream, peer) = Listener::accept(&mut self.listener).await;
        let permit = self.connections.acquire().await;
        (PermitStream::new(stream, permit), peer)
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

#[derive(Clone)]
pub(super) struct LimitedAcceptor<A> {
    inner: A,
    connections: ConnectionGate,
}

impl<A> LimitedAcceptor<A> {
    pub(super) fn new(inner: A, connections: ConnectionGate) -> Self {
        Self { inner, connections }
    }
}

impl<I, S, A> Accept<I, S> for LimitedAcceptor<A>
where
    A: Accept<I, S>,
    A::Future: Send + 'static,
    A::Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    A::Service: 'static,
{
    type Stream = PermitStream<A::Stream>;
    type Service = A::Service;
    type Future =
        Pin<Box<dyn Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send + 'static>>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let Some(permit) = self.connections.try_acquire() else {
            return Box::pin(async {
                Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "HTTP connection limit reached",
                ))
            });
        };
        let accepted = self.inner.accept(stream, service);
        Box::pin(async move {
            let (stream, service) = accepted.await?;
            Ok((PermitStream::new(stream, permit), service))
        })
    }
}

pub(super) struct PermitStream<T> {
    inner: T,
    _permit: OwnedSemaphorePermit,
}

impl<T> PermitStream<T> {
    fn new(inner: T, permit: OwnedSemaphorePermit) -> Self {
        Self {
            inner,
            _permit: permit,
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for PermitStream<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(context, buffer)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for PermitStream<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_write(context, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(context)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(context)
    }
}

pub(super) fn limit_http_requests(
    app: Router,
    timeout: Duration,
    body_limit: NonZeroUsize,
) -> Router {
    app.layer(DefaultBodyLimit::max(body_limit.get()))
        .layer(middleware::from_fn_with_state(timeout, request_timeout))
}

async fn request_timeout(
    State(timeout): State<Duration>,
    request: Request,
    next: Next,
) -> Response {
    match tokio::time::timeout(timeout, next.run(request)).await {
        Ok(response) => response,
        Err(_) => (axum::http::StatusCode::REQUEST_TIMEOUT, "request timed out").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connection_gate_waits_until_a_connection_closes() -> Result<(), io::Error> {
        let gate = ConnectionGate::new(NonZeroUsize::MIN);
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;

        let first_client = TcpStream::connect(address).await?;
        let first = gate.accept(&listener).await?;
        let second_client = TcpStream::connect(address).await?;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), gate.accept(&listener))
                .await
                .is_err()
        );

        drop(first);
        drop(first_client);
        drop(second_client);
        let third_client = TcpStream::connect(address).await?;
        let third = tokio::time::timeout(Duration::from_secs(1), gate.accept(&listener))
            .await
            .map_err(io::Error::other)??;
        drop(third);
        drop(third_client);
        Ok(())
    }
}
