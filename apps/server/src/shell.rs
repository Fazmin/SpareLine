//! Panel layout: the header with the status chip, page tabs, page routing, overview polling.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::panel::{CheckStatus, ServerStatus};
use sl_ui::{AaButton, Icon, Mark, SignalLine, ipc};

use crate::ctx::{OverviewData, Page, call0, ctx};

pub fn poll_overview() {
    let c = ctx();
    spawn_local(async move {
        if let Ok(o) = call0::<OverviewData>("overview").await {
            c.overview.set(Some(o));
        }
    });
}

/// How the server's state is named and drawn: `(data-s value, icon, words)`.
/// The status chip and the overview sign both use it, so the state is never colour alone.
pub fn status_look(s: ServerStatus) -> (&'static str, &'static str, &'static str) {
    match s {
        ServerStatus::Running => ("running", "check", "Running"),
        ServerStatus::Degraded => ("degraded", "alert", "Running with problems"),
        ServerStatus::Stopped => ("stopped", "close", "Stopped"),
    }
}

#[component]
pub fn Shell() -> impl IntoView {
    let c = ctx();
    poll_overview();
    if let Ok(h) = set_interval_with_handle(poll_overview, std::time::Duration::from_secs(2)) {
        on_cleanup(move || h.clear());
    }
    let line_state = Signal::derive(move || match c.overview.get().map(|o| o.o.status) {
        Some(ServerStatus::Running) => "connected".to_string(),
        Some(ServerStatus::Degraded) => "reconnecting".to_string(),
        Some(ServerStatus::Stopped) => "offline".to_string(),
        None => "idle".to_string(),
    });
    let line_label = Signal::derive(move || match c.overview.get().map(|o| o.o.status) {
        Some(ServerStatus::Running) => "Server running".to_string(),
        Some(ServerStatus::Degraded) => "Server running with problems".to_string(),
        Some(ServerStatus::Stopped) => "Server stopped".to_string(),
        None => String::new(),
    });
    let server_name = Memo::new(move |_| {
        c.overview
            .with(|o| o.as_ref().map(|d| d.o.server_name.clone()))
            .unwrap_or_default()
    });
    let chip = Memo::new(move |_| {
        c.overview
            .with(|o| o.as_ref().map(|d| (d.o.status, d.o.users_online)))
    });
    let to_check = Memo::new(move |_| {
        c.overview
            .with(|o| {
                o.as_ref().map(|d| {
                    d.o.health.iter().fold((0, 0), |(f, w), h| match h.status {
                        CheckStatus::Fail => (f + 1, w),
                        CheckStatus::Warn => (f, w + 1),
                        _ => (f, w),
                    })
                })
            })
            .unwrap_or((0, 0))
    });
    let live = Memo::new(move |_| {
        c.overview
            .with(|o| o.as_ref().map(|d| d.o.active_broadcasts))
            .unwrap_or(0)
    });

    let tab = move |p: Page, icon: &'static str, label: &'static str| {
        view! {
            <button class="tab" aria-current=move || (c.page.get() == p).then_some("page") on:click=move |_| c.page.set(p)>
                <Icon name=icon/>
                <span>{label}</span>
                {(p == Page::Diagnostics).then(|| view! {
                    {move || {
                        let (fails, warns) = to_check.get();
                        let n = fails + warns;
                        (n > 0).then(|| view! {
                            <span class={if fails > 0 { "tag fail" } else { "tag warn" }} aria-label=format!("{n} to check")>{n}</span>
                        })
                    }}
                })}
                {(p == Page::Announcements).then(|| view! {
                    {move || {
                        let n = live.get();
                        (n > 0).then(|| view! { <span class="tag ok" aria-label=format!("{n} live")>"Live"</span> })
                    }}
                })}
            </button>
        }
    };
    let lock = move |_| {
        spawn_local(async move {
            #[derive(Serialize)]
            struct No {}
            let _ = ipc::invoke::<()>("lock", No {}).await;
            c.locked();
        });
    };
    view! {
        <SignalLine state=line_state label=line_label/>
        <a class="skip-link" href="#main">"Skip to content"</a>
        <div class="panel-app">
            <header class="topbar">
                <div class="topbar-row">
                    <div class="brand">
                        <Mark/>
                        <span>"SpareLine Server"</span>
                        <small class="truncate">{move || server_name.get()}</small>
                    </div>
                    {move || chip.get().map(|(s, online)| {
                        let (state, icon, words) = status_look(s);
                        view! {
                            <button class="status-chip" data-s=state title="Open the overview" on:click=move |_| c.page.set(Page::Overview)>
                                <Icon name=icon/>
                                <span>{words}</span>
                                {(s != ServerStatus::Stopped).then(|| view! { <span class="chip-detail">{format!("{online} connected")}</span> })}
                            </button>
                        }
                    })}
                    <span class="grow"></span>
                    <AaButton/>
                    <button class="btn quiet small" on:click=lock><Icon name="lock"/><span class="label-text">"Lock panel"</span></button>
                </div>
                <nav class="tabs" aria-label="Control panel">
                    {tab(Page::Overview, "home", "Overview")}
                    {tab(Page::People, "people", "People")}
                    {tab(Page::Invites, "invite", "Invites")}
                    {tab(Page::Activity, "pulse", "Activity")}
                    {tab(Page::Diagnostics, "tool", "Diagnostics")}
                    {tab(Page::Groups, "groups", "Groups")}
                    {tab(Page::Announcements, "megaphone", "Announcements")}
                    {tab(Page::Settings, "settings", "Settings")}
                </nav>
            </header>
            <main class="main" id="main" tabindex="-1">
                <div class="main-inner">
                    {move || match c.page.get() {
                        Page::Overview => view! { <crate::overview::OverviewPage/> }.into_any(),
                        Page::People => view! { <crate::people::PeoplePage/> }.into_any(),
                        Page::Groups => view! { <crate::groups::GroupsPage/> }.into_any(),
                        Page::Invites => view! { <crate::invites::InvitesPage/> }.into_any(),
                        Page::Announcements => view! { <crate::announcements::AnnouncementsPage/> }.into_any(),
                        Page::Activity => view! { <crate::activity::ActivityPage/> }.into_any(),
                        Page::Diagnostics => view! { <crate::diagnostics::DiagnosticsPage/> }.into_any(),
                        Page::Settings => view! { <crate::settings::SettingsPage/> }.into_any(),
                    }}
                </div>
            </main>
        </div>
    }
}
