//! TLS listener for axum. Handshakes run concurrently (one slow client can't block
//! accepts) and failures are recorded as connection events for diagnostics.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::connect_info::Connected;
use axum::serve::{IncomingStream, Listener};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

/// Called for each TLS failure: (peer, plain-language reason).
pub type TlsFailureHook = Arc<dyn Fn(SocketAddr, String) + Send + Sync>;

pub struct TlsListener {
    rx: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    local: SocketAddr,
    _accept_task: AbortOnDrop,
}

struct AbortOnDrop(JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl TlsListener {
    pub async fn bind(
        addr: SocketAddr,
        config: Arc<rustls::ServerConfig>,
        on_failure: TlsFailureHook,
    ) -> std::io::Result<Self> {
        let tcp = TcpListener::bind(addr).await?;
        let local = tcp.local_addr()?;
        let acceptor = TlsAcceptor::from(config);
        let (tx, rx) = mpsc::channel(128);
        let task = tokio::spawn(async move {
            loop {
                let (stream, peer) = match tcp.accept().await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(target: "sl_server::tls", error = %e, "accept failed");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let _ = stream.set_nodelay(true);
                let acceptor = acceptor.clone();
                let tx = tx.clone();
                let on_failure = on_failure.clone();
                tokio::spawn(async move {
                    match tokio::time::timeout(Duration::from_secs(10), acceptor.accept(stream))
                        .await
                    {
                        Ok(Ok(tls)) => {
                            tracing::trace!(target: "sl_server::tls", %peer, "TLS handshake ok");
                            let _ = tx.send((tls, peer)).await;
                        }
                        Ok(Err(e)) => {
                            let reason = explain_tls_error(&e.to_string());
                            tracing::debug!(target: "sl_server::tls", %peer, error = %e, "TLS handshake failed");
                            on_failure(peer, reason);
                        }
                        Err(_) => {
                            tracing::debug!(target: "sl_server::tls", %peer, "TLS handshake timed out");
                            on_failure(peer, "TLS handshake timed out (client stalled or not a SpareLine client)".into());
                        }
                    }
                });
            }
        });
        Ok(Self {
            rx,
            local,
            _accept_task: AbortOnDrop(task),
        })
    }
}

/// Turn rustls errors into something an admin can act on.
pub fn explain_tls_error(e: &str) -> String {
    if e.contains("BadCertificate") || e.contains("UnknownCA") || e.contains("CertificateUnknown") {
        "Client rejected the server certificate: its pinned fingerprint doesn't match (server cert was regenerated?) — issue the user a setup code".into()
    } else if e.contains("peer is incompatible")
        || e.contains("NoCipherSuitesInCommon")
        || e.contains("ProtocolVersion")
    {
        "Client uses an incompatible TLS version (very old or non-SpareLine client)".into()
    } else if e.contains("InvalidContentType") || e.contains("corrupt message") {
        "Non-TLS traffic on the SpareLine port (a browser using http://, or a port scanner)".into()
    } else if e.contains("unexpected end of file")
        || e.contains("connection reset")
        || e.contains("UnexpectedEof")
    {
        "Client disconnected during the TLS handshake".into()
    } else {
        format!("TLS handshake failed: {e}")
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(v) => v,
            // The accept task ended (shutdown): never resolve.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// Peer address extractor for handlers served by [`TlsListener`].
#[derive(Debug, Clone, Copy)]
pub struct PeerAddr(pub SocketAddr);

impl Connected<IncomingStream<'_, TlsListener>> for PeerAddr {
    fn connect_info(stream: IncomingStream<'_, TlsListener>) -> Self {
        Self(*stream.remote_addr())
    }
}
