//! Panel-wide state and the command helper.

use leptos::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sl_proto::client_ipc::UiError;
use sl_proto::panel::{Overview, PanelState};
use sl_ui::ipc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Overview,
    People,
    Groups,
    Invites,
    Announcements,
    Activity,
    Diagnostics,
    Settings,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct OverviewData {
    #[serde(flatten)]
    pub o: Overview,
    pub start_error: Option<String>,
}

#[derive(Clone, Copy)]
pub struct Ctx {
    pub state: RwSignal<Option<PanelState>>,
    pub page: RwSignal<Page>,
    pub overview: RwSignal<Option<OverviewData>>,
    /// Ask the Invites page to open its create form.
    pub invite_now: RwSignal<bool>,
    /// Ask the People page to select someone.
    pub focus_user: RwSignal<Option<uuid::Uuid>>,
}

thread_local! {
    /// The panel has exactly one `Ctx`. Async tasks (`spawn_local`) and timers run without
    /// a reactive owner, so `use_context` finds nothing there; they fall back to this copy.
    static PANEL: std::cell::Cell<Option<Ctx>> = const { std::cell::Cell::new(None) };
}

pub fn ctx() -> Ctx {
    use_context::<Ctx>()
        .or_else(|| PANEL.with(std::cell::Cell::get))
        .expect("the panel context is created in App before any page renders")
}

impl Ctx {
    pub fn new() -> Self {
        let c = Self {
            state: RwSignal::new(None),
            page: RwSignal::new(Page::Overview),
            overview: RwSignal::new(None),
            invite_now: RwSignal::new(false),
            focus_user: RwSignal::new(None),
        };
        PANEL.with(|p| p.set(Some(c)));
        c
    }

    pub fn locked(&self) {
        self.state.update(|s| {
            if let Some(s) = s {
                s.unlocked = false;
            }
        });
    }
}

/// Call a panel command; a "locked" answer returns the panel to the unlock screen.
pub async fn call<T: DeserializeOwned>(cmd: &str, args: impl Serialize) -> Result<T, UiError> {
    let r = ipc::invoke::<T>(cmd, args).await;
    if let Err(e) = &r
        && e.code == "locked"
    {
        ctx().locked();
    }
    r
}

pub async fn call0<T: DeserializeOwned>(cmd: &str) -> Result<T, UiError> {
    #[derive(Serialize)]
    struct No {}
    call(cmd, No {}).await
}

/// Fire a command and toast the outcome.
pub fn act(
    cmd: &'static str,
    args: impl Serialize + 'static,
    ok: &'static str,
    then: impl Fn() + 'static,
) {
    leptos::task::spawn_local(async move {
        match call::<serde_json::Value>(cmd, args).await {
            Ok(_) => {
                if !ok.is_empty() {
                    sl_ui::toasts().info(ok);
                }
                then();
            }
            Err(e) => sl_ui::toasts().error(e.message),
        }
    });
}
