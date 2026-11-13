//! WebSocket endpoint: one task per connection.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use sl_proto::panel::ConnEventKind;
use sl_proto::wire::{ClientFrame, ServerFrame};
use sl_proto::{ApiError, ConversationId, ErrorCode, Presence};
use tokio::sync::mpsc;

use crate::Engine;
use crate::api::{self, Session};
use crate::delivery::Caller;
use crate::hub::{Conn, OUTBOUND_CAPACITY, Outbound, encode};
use crate::net::PeerAddr;

const MAX_MESSAGE: usize = 2 * 1024 * 1024;
const AUTH_TIMEOUT: Duration = Duration::from_secs(60);
const PING_EVERY: Duration = Duration::from_secs(20);
const IDLE_TIMEOUT: Duration = Duration::from_secs(70);
const MAX_AUTH_FAILURES: u32 = 5;
const TYPING_MIN_GAP: Duration = Duration::from_millis(2000);

pub async fn route(
    State(engine): State<Engine>,
    ConnectInfo(PeerAddr(peer)): ConnectInfo<PeerAddr>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| run(engine, socket, peer))
}

async fn send_frame(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    frame: &ServerFrame,
) -> bool {
    let text = encode(frame);
    sink.send(Message::Text(text.to_string().into()))
        .await
        .is_ok()
}

async fn run(engine: Engine, socket: WebSocket, peer: SocketAddr) {
    let (mut sink, mut stream) = socket.split();
    if !send_frame(
        &mut sink,
        &ServerFrame::Hello {
            server: engine.server_info(),
        },
    )
    .await
    {
        return;
    }

    // ---- Phase 1: authenticate -------------------------------------------------------
    let deadline = tokio::time::Instant::now() + AUTH_TIMEOUT;
    let mut failures = 0u32;
    let authed = loop {
        let msg = match tokio::time::timeout_at(deadline, stream.next()).await {
            Err(_) => {
                tracing::debug!(target: "sl_server::ws", %peer, "no sign-in within timeout");
                let _ = sink.close().await;
                return;
            }
            Ok(None) | Ok(Some(Err(_))) => return,
            Ok(Some(Ok(m))) => m,
        };
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => return,
            _ => continue,
        };
        let frame: ClientFrame = match serde_json::from_str(text.as_str()) {
            Ok(f) => f,
            Err(e) => {
                tracing::debug!(target: "sl_server::ws", %peer, error = %e, "unparseable frame before sign-in");
                failures += 1;
                let _ = send_frame(
                    &mut sink,
                    &ServerFrame::AuthError(ApiError::new(ErrorCode::BadRequest)),
                )
                .await;
                if failures >= MAX_AUTH_FAILURES {
                    return;
                }
                continue;
            }
        };
        let result = match frame {
            ClientFrame::Prove { invite_id, nonce } => {
                let reply = match crate::auth::prove(&engine, invite_id, nonce, peer).await {
                    Ok(proof) => ServerFrame::Proof { proof },
                    Err(e) => ServerFrame::AuthError(e.into_api()),
                };
                if !send_frame(&mut sink, &reply).await {
                    return;
                }
                continue;
            }
            ClientFrame::Ping { ts } => {
                let _ = send_frame(&mut sink, &ServerFrame::Pong { ts }).await;
                continue;
            }
            ClientFrame::Register(r) => crate::auth::register(&engine, r, peer).await,
            ClientFrame::Redeem(r) => crate::auth::redeem(&engine, r, peer).await,
            ClientFrame::Login(r) => crate::auth::login(&engine, r, peer).await,
            ClientFrame::Resume(r) => crate::auth::resume(&engine, r, peer).await,
            _ => Err(ErrorCode::NotAuthenticated.into()),
        };
        match result {
            Ok(a) => break a,
            Err(e) => {
                let api = e.into_api();
                if matches!(
                    api.code,
                    ErrorCode::ProtocolTooOld | ErrorCode::ProtocolTooNew
                ) {
                    engine.conn_event(
                        peer,
                        None,
                        None,
                        ConnEventKind::ProtocolMismatch,
                        format!("{:?}", api.code),
                    );
                }
                failures += 1;
                if !send_frame(&mut sink, &ServerFrame::AuthError(api)).await
                    || failures >= MAX_AUTH_FAILURES
                {
                    return;
                }
            }
        }
    };

    // ---- Phase 2: authenticated -------------------------------------------------------
    let (tx, mut rx) = mpsc::channel::<Outbound>(OUTBOUND_CAPACITY);
    // Authed must be the first frame: queue it before registering for events.
    let _ = tx.try_send(Outbound::Text(encode(&ServerFrame::Authed(Box::new(
        authed.ok.clone(),
    )))));
    let conn = Arc::new(Conn {
        id: engine.hub().next_conn_id(),
        user_id: authed.user_id,
        device_id: authed.device_id,
        username: authed.username.clone(),
        display_name: authed.display_name.clone(),
        device_name: authed.device_name.clone(),
        platform: authed.platform.clone(),
        client_version: authed.client_version.clone(),
        ip: peer.ip().to_string(),
        connected_at: sl_proto::now_ms(),
        rtt_ms: Default::default(),
        presence: Mutex::new(Presence::Online),
        tx: tx.clone(),
    });
    let was_online = engine.hub().is_user_online(&authed.user_id);
    if let Some(old) = engine.hub().register(conn.clone()) {
        let _ = old.tx.try_send(Outbound::Close(ApiError::with_detail(
            ErrorCode::SessionRevoked,
            "replaced by a newer connection from the same device",
        )));
    }
    engine
        .tel()
        .metrics
        .set_connections(engine.hub().count() as u32);
    engine.conn_event(
        peer,
        Some(authed.user_id),
        Some(authed.device_id),
        ConnEventKind::Connected,
        format!("{} {}", authed.platform, authed.client_version),
    );
    tracing::info!(target: "sl_server::ws", user = %authed.username, device = %authed.device_name, %peer, "connected");
    if !was_online {
        api::broadcast_presence(&engine, authed.user_id);
    }

    let ping_sent: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let ping_sent_w = ping_sent.clone();
    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.tick().await;
        loop {
            tokio::select! {
                m = rx.recv() => match m {
                    Some(Outbound::Text(t)) => {
                        if sink.send(Message::Text(t.to_string().into())).await.is_err() { break; }
                    }
                    Some(Outbound::Close(err)) => {
                        let _ = sink.send(Message::Text(encode(&ServerFrame::Close(err)).to_string().into())).await;
                        let _ = sink.send(Message::Close(Some(CloseFrame { code: 1000, reason: "closing".into() }))).await;
                        break;
                    }
                    None => break,
                },
                _ = ping.tick() => {
                    *ping_sent_w.lock() = Some(Instant::now());
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
                }
            }
        }
        let _ = sink.close().await;
    });

    let session = Session {
        conn_id: conn.id,
        caller: Caller {
            user: authed.user_id,
            device: authed.device_id,
        },
        token_hash: authed.token_hash.clone(),
    };
    let mut last_typing: HashMap<ConversationId, Instant> = HashMap::new();
    let mut close_reason = "client closed";

    loop {
        let msg = match tokio::time::timeout(IDLE_TIMEOUT, stream.next()).await {
            Err(_) => {
                close_reason = "idle timeout (no pong)";
                break;
            }
            Ok(None) => break,
            Ok(Some(Err(_))) => {
                close_reason = "connection error";
                break;
            }
            Ok(Some(Ok(m))) => m,
        };
        if writer.is_finished() {
            close_reason = "writer closed";
            break;
        }
        match msg {
            Message::Text(text) => {
                let frame: ClientFrame = match serde_json::from_str(text.as_str()) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::debug!(target: "sl_server::ws", user = %authed.username, error = %e, "bad frame");
                        continue;
                    }
                };
                match frame {
                    ClientFrame::Req { id, body } => {
                        let is_logout = matches!(body, sl_proto::wire::Request::Logout);
                        let op = request_name(&body);
                        let started = Instant::now();
                        let result = api::dispatch(&engine, &session, body)
                            .await
                            .map_err(|e| e.into_api());
                        if let Err(e) = &result {
                            tracing::debug!(target: "sl_server::ws", user = %authed.username, op, code = ?e.code, "request failed");
                        } else {
                            tracing::trace!(target: "sl_server::ws", user = %authed.username, op, ms = started.elapsed().as_millis() as u64, "request ok");
                        }
                        if tx
                            .send(Outbound::Text(encode(&ServerFrame::Res { id, result })))
                            .await
                            .is_err()
                        {
                            break;
                        }
                        if is_logout {
                            close_reason = "signed out";
                            let _ = tx
                                .send(Outbound::Close(ApiError::with_detail(
                                    ErrorCode::SessionRevoked,
                                    "signed out",
                                )))
                                .await;
                            break;
                        }
                    }
                    ClientFrame::Typing { conversation } => {
                        let now = Instant::now();
                        let ok = last_typing
                            .get(&conversation)
                            .is_none_or(|t| now.duration_since(*t) >= TYPING_MIN_GAP);
                        if ok {
                            last_typing.insert(conversation, now);
                            let _ = crate::delivery::typing(&engine, session.caller, conversation)
                                .await;
                        }
                    }
                    ClientFrame::Ping { ts } => {
                        let _ = tx
                            .send(Outbound::Text(encode(&ServerFrame::Pong { ts })))
                            .await;
                    }
                    _ => {
                        let _ = tx
                            .send(Outbound::Text(encode(&ServerFrame::AuthError(
                                ApiError::with_detail(ErrorCode::BadRequest, "already signed in"),
                            ))))
                            .await;
                    }
                }
            }
            Message::Pong(_) => {
                if let Some(t) = ping_sent.lock().take() {
                    let ms = t.elapsed().as_millis().clamp(1, u32::MAX as u128) as u32;
                    conn.rtt_ms.store(ms, Ordering::Relaxed);
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // ---- Cleanup ---------------------------------------------------------------------
    engine.hub().unregister(conn.id);
    drop(tx);
    writer.abort();
    engine
        .tel()
        .metrics
        .set_connections(engine.hub().count() as u32);
    engine.conn_event(
        peer,
        Some(authed.user_id),
        Some(authed.device_id),
        ConnEventKind::Disconnected,
        close_reason,
    );
    tracing::info!(target: "sl_server::ws", user = %authed.username, device = %authed.device_name, reason = close_reason, "disconnected");
    if !engine.hub().is_user_online(&authed.user_id) {
        api::broadcast_presence(&engine, authed.user_id);
    }
}

fn request_name(r: &sl_proto::wire::Request) -> &'static str {
    use sl_proto::wire::Request::*;
    match r {
        Directory => "directory",
        Conversations => "conversations",
        Sync { .. } => "sync",
        Ack { .. } => "ack",
        UploadKeyPackages { .. } => "upload_key_packages",
        ClaimKeyPackages { .. } => "claim_key_packages",
        CreateConversation { .. } => "create_conversation",
        Commit { .. } => "commit",
        Send { .. } => "send",
        Fetch { .. } => "fetch",
        GroupInfo { .. } => "group_info",
        ExternalJoin { .. } => "external_join",
        Purge { .. } => "purge",
        Leave { .. } => "leave",
        MarkRead { .. } => "mark_read",
        SetPresence { .. } => "set_presence",
        SetDisplayName { .. } => "set_display_name",
        ChangePassword { .. } => "change_password",
        ReportHealth(_) => "report_health",
        Logout => "logout",
    }
}
