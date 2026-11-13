//! Authenticated WebSocket transport: request/response correlation and an event stream.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use sl_net::WsStream;
use sl_net::tokio_tungstenite::tungstenite::Message;
use sl_proto::ApiError;
use sl_proto::wire::{ClientFrame, Event, Request, Response, ServerFrame};
use tokio::sync::{mpsc, oneshot};

use crate::error::{ClientError, ClientResult};

const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const PING_EVERY: Duration = Duration::from_secs(15);
const DEAD_AFTER: Duration = Duration::from_secs(50);

/// What the transport reports to the client core.
#[derive(Debug)]
pub enum Incoming {
    Event(Event),
    Rtt(u32),
    /// Connection ended. `Some` if the server said why.
    Closed {
        generation: u64,
        reason: Option<ApiError>,
    },
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Response, ApiError>>>>>;

#[derive(Clone)]
pub struct Transport {
    out: mpsc::UnboundedSender<ClientFrame>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
    pub generation: u64,
}

// ---- pre-auth helpers on a raw socket ------------------------------------------------

pub async fn send_raw(ws: &mut WsStream, frame: &ClientFrame) -> ClientResult<()> {
    let text = serde_json::to_string(frame)?;
    ws.send(Message::Text(text.into()))
        .await
        .map_err(|e| ClientError::Connect(format!("connection lost: {e}")))
}

pub async fn recv_raw(ws: &mut WsStream, timeout: Duration) -> ClientResult<ServerFrame> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next())
            .await
            .map_err(|_| ClientError::Connect("The server didn't respond in time.".into()))?;
        match msg {
            Some(Ok(Message::Text(t))) => {
                return serde_json::from_str(t.as_str()).map_err(|e| {
                    ClientError::Connect(format!("unexpected reply from server: {e}"))
                });
            }
            Some(Ok(Message::Close(_))) | None => {
                return Err(ClientError::Connect(
                    "The server closed the connection.".into(),
                ));
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(ClientError::Connect(format!("connection lost: {e}"))),
        }
    }
}

impl Transport {
    /// Take over an authenticated socket.
    pub fn start(ws: WsStream, generation: u64, incoming: mpsc::UnboundedSender<Incoming>) -> Self {
        let (mut sink, mut stream) = ws.split();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ClientFrame>();
        let pending: Pending = Arc::default();

        // Writer + keepalive. Ends when every Transport clone is dropped.
        tokio::spawn(async move {
            let mut ping = tokio::time::interval(PING_EVERY);
            ping.tick().await;
            loop {
                tokio::select! {
                    f = out_rx.recv() => {
                        let Some(f) = f else { break };
                        let Ok(text) = serde_json::to_string(&f) else { continue };
                        if sink.send(Message::Text(text.into())).await.is_err() { break; }
                    }
                    _ = ping.tick() => {
                        let Ok(text) = serde_json::to_string(&ClientFrame::Ping { ts: sl_proto::now_ms() }) else { continue };
                        if sink.send(Message::Text(text.into())).await.is_err() { break; }
                    }
                }
            }
            let _ = sink.close().await;
        });

        // Reader.
        let pend = pending.clone();
        tokio::spawn(async move {
            let mut reason = None;
            loop {
                let msg = match tokio::time::timeout(DEAD_AFTER, stream.next()).await {
                    Err(_) => {
                        tracing::warn!("connection silent for too long; reconnecting");
                        break;
                    }
                    Ok(None) | Ok(Some(Err(_))) => break,
                    Ok(Some(Ok(m))) => m,
                };
                let Message::Text(text) = msg else {
                    if matches!(msg, Message::Close(_)) {
                        break;
                    }
                    continue;
                };
                let frame: ServerFrame = match serde_json::from_str(text.as_str()) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!(error = %e, "unparseable frame from server");
                        continue;
                    }
                };
                match frame {
                    ServerFrame::Res { id, result } => {
                        if let Some(tx) = pend.lock().remove(&id) {
                            let _ = tx.send(result);
                        }
                    }
                    ServerFrame::Event(e) => {
                        let _ = incoming.send(Incoming::Event(e));
                    }
                    ServerFrame::Pong { ts } => {
                        let rtt = (sl_proto::now_ms() - ts).clamp(0, u32::MAX as i64) as u32;
                        let _ = incoming.send(Incoming::Rtt(rtt.max(1)));
                    }
                    ServerFrame::Close(e) => {
                        reason = Some(e);
                        break;
                    }
                    _ => {}
                }
            }
            // Fail everything still waiting.
            for (_, tx) in pend.lock().drain() {
                let _ = tx.send(Err(ApiError::new(sl_proto::ErrorCode::ServerUnreachable)));
            }
            let _ = incoming.send(Incoming::Closed { generation, reason });
        });

        Self {
            out: out_tx,
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
            generation,
        }
    }

    pub async fn call(&self, body: Request) -> ClientResult<Response> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        if self.out.send(ClientFrame::Req { id, body }).is_err() {
            self.pending.lock().remove(&id);
            return Err(ClientError::Offline);
        }
        match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(Ok(r))) => Ok(r),
            Ok(Ok(Err(e))) if e.code == sl_proto::ErrorCode::ServerUnreachable => {
                Err(ClientError::Offline)
            }
            Ok(Ok(Err(e))) => Err(ClientError::Api(e)),
            Ok(Err(_)) => Err(ClientError::Offline),
            Err(_) => {
                self.pending.lock().remove(&id);
                Err(ClientError::Connect(
                    "The server took too long to answer.".into(),
                ))
            }
        }
    }

    pub fn send(&self, frame: ClientFrame) {
        let _ = self.out.send(frame);
    }

    pub fn is_open(&self) -> bool {
        !self.out.is_closed()
    }
}
