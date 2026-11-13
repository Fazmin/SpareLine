//! SpareLine client engine.
//!
//! [`Client`] is a cheap, thread-safe handle. All state lives in a single-threaded core
//! (its own thread and Tokio runtime) that owns the MLS group state and the local store.

mod core;
pub mod discovery;
pub mod error;
pub mod mls;
pub mod payload;
pub mod profile;
pub mod store;
pub mod transport;

use std::sync::Arc;

use futures_util::future::LocalBoxFuture;
use parking_lot::RwLock;
use sl_proto::client_ipc::*;
use sl_proto::{ConversationId, Directory, Presence, UserId};
use tokio::sync::{broadcast, mpsc, oneshot};
use uuid::Uuid;

pub use crate::core::{ClientOptions, Core};
pub use crate::error::{ClientError, ClientResult};
pub use crate::payload::QUICK_REACTIONS;
pub use crate::profile::{FileStore, KeyringStore, MemoryStore, SecretStore};

pub(crate) type Job = Box<dyn for<'a> FnOnce(&'a mut Core) -> LocalBoxFuture<'a, ()> + Send>;

#[derive(Clone)]
pub struct Client {
    jobs: mpsc::UnboundedSender<Job>,
    events: broadcast::Sender<ClientEvent>,
    state: Arc<RwLock<AppState>>,
}

impl Client {
    /// Start the client core on its own thread.
    pub fn start(opts: ClientOptions) -> anyhow::Result<Self> {
        let (jobs_tx, jobs_rx) = mpsc::unbounded_channel::<Job>();
        let (events, _) = broadcast::channel(2048);
        let state = Arc::new(RwLock::new(AppState {
            phase: Phase::Onboarding,
            me: None,
            server: None,
            conn: ConnState::Offline,
            broadcasts: vec![],
        }));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();
        let ev = events.clone();
        let st = state.clone();
        std::thread::Builder::new()
            .name("spareline-core".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.into()));
                        return;
                    }
                };
                rt.block_on(async move {
                    match Core::open(opts, ev, st) {
                        Ok(core) => {
                            let _ = ready_tx.send(Ok(()));
                            core.run(jobs_rx).await;
                        }
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                        }
                    }
                });
            })?;
        ready_rx.recv()??;
        Ok(Self {
            jobs: jobs_tx,
            events,
            state,
        })
    }

    async fn call<T, F>(&self, f: F) -> ClientResult<T>
    where
        T: Send + 'static,
        F: for<'a> FnOnce(&'a mut Core) -> LocalBoxFuture<'a, ClientResult<T>> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let job: Job = Box::new(move |core| {
            Box::pin(async move {
                let r = f(core).await;
                let _ = tx.send(r);
            })
        });
        self.jobs
            .send(job)
            .map_err(|_| ClientError::Internal("client stopped".into()))?;
        rx.await
            .map_err(|_| ClientError::Internal("client stopped".into()))?
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ClientEvent> {
        self.events.subscribe()
    }

    pub fn state(&self) -> AppState {
        self.state.read().clone()
    }

    pub async fn discover(&self) -> ClientResult<Vec<DiscoveredServer>> {
        self.call(|c| Box::pin(async move { Ok(c.discover().await) }))
            .await
    }
    pub async fn join(&self, input: JoinInput) -> ClientResult<AppState> {
        self.call(move |c| Box::pin(c.join(input))).await
    }
    pub async fn redeem(&self, input: RedeemInput) -> ClientResult<AppState> {
        self.call(move |c| Box::pin(c.redeem(input))).await
    }
    pub async fn login(&self, input: LoginInput) -> ClientResult<AppState> {
        self.call(move |c| Box::pin(c.login(input))).await
    }
    pub async fn logout(&self, forget: bool) -> ClientResult<AppState> {
        self.call(move |c| Box::pin(c.logout(forget))).await
    }
    pub async fn reconnect_now(&self) -> ClientResult<()> {
        self.call(|c| {
            Box::pin(async move {
                c.reconnect_now();
                Ok(())
            })
        })
        .await
    }
    pub async fn conversations(&self) -> ClientResult<Vec<ConversationView>> {
        self.call(|c| Box::pin(async move { Ok(c.list_conversations()) }))
            .await
    }
    pub async fn messages(
        &self,
        conv: ConversationId,
        before: Option<i64>,
        limit: u32,
    ) -> ClientResult<Vec<MessageView>> {
        self.call(move |c| Box::pin(async move { Ok(c.messages(conv, before, limit)) }))
            .await
    }
    pub async fn send(&self, input: SendInput) -> ClientResult<MessageView> {
        self.call(move |c| Box::pin(async move { c.send_text(input) }))
            .await
    }
    pub async fn edit(&self, conv: ConversationId, id: Uuid, body: String) -> ClientResult<()> {
        self.call(move |c| Box::pin(async move { c.edit(conv, id, body) }))
            .await
    }
    pub async fn delete(&self, conv: ConversationId, id: Uuid) -> ClientResult<()> {
        self.call(move |c| Box::pin(async move { c.delete(conv, id) }))
            .await
    }
    pub async fn react(&self, conv: ConversationId, id: Uuid, emoji: String) -> ClientResult<()> {
        self.call(move |c| Box::pin(async move { c.react(conv, id, emoji) }))
            .await
    }
    pub async fn mark_read(&self, conv: ConversationId) -> ClientResult<()> {
        self.call(move |c| Box::pin(c.mark_read(conv))).await
    }
    pub async fn typing(&self, conv: ConversationId) -> ClientResult<()> {
        self.call(move |c| {
            Box::pin(async move {
                c.typing(conv);
                Ok(())
            })
        })
        .await
    }
    pub async fn start_chat(&self, input: StartChatInput) -> ClientResult<ConversationView> {
        self.call(move |c| Box::pin(c.start_chat(input))).await
    }
    pub async fn add_people(&self, conv: ConversationId, users: Vec<UserId>) -> ClientResult<()> {
        self.call(move |c| Box::pin(c.add_people(conv, users)))
            .await
    }
    pub async fn leave(&self, conv: ConversationId) -> ClientResult<()> {
        self.call(move |c| Box::pin(c.leave(conv))).await
    }
    pub async fn directory(&self) -> ClientResult<Directory> {
        self.call(|c| Box::pin(c.directory())).await
    }
    pub async fn set_presence(&self, p: Presence) -> ClientResult<()> {
        self.call(move |c| Box::pin(c.set_presence(p))).await
    }
    pub async fn set_display_name(&self, name: String) -> ClientResult<()> {
        self.call(move |c| Box::pin(c.set_display_name(name))).await
    }
    pub async fn change_password(&self, current: String, new: String) -> ClientResult<()> {
        self.call(move |c| Box::pin(c.change_password(current, new)))
            .await
    }
    pub async fn set_read_receipts(&self, on: bool) -> ClientResult<()> {
        self.call(move |c| Box::pin(async move { c.set_read_receipts(on) }))
            .await
    }
    pub async fn read_receipts(&self) -> ClientResult<bool> {
        self.call(|c| Box::pin(async move { Ok(c.read_receipts()) }))
            .await
    }
    pub async fn search(&self, q: String) -> ClientResult<Vec<SearchHit>> {
        self.call(move |c| Box::pin(async move { Ok(c.search(&q)) }))
            .await
    }
    pub async fn diagnostics(&self) -> ClientResult<Diagnostics> {
        self.call(|c| Box::pin(async move { Ok(c.diagnostics()) }))
            .await
    }
}
