//! Panel layout: navigation, the status line, page routing, overview polling.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::panel::ServerStatus;
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
    let btn = move |p: Page, icon: &'static str, label: &'static str| {
        view! {
            <button class="nav-btn" aria-current=move || (c.page.get() == p).then_some("page") on:click=move |_| c.page.set(p) title=label>
                <Icon name=icon/>
                <span>{label}</span>
                {(p == Page::Diagnostics).then(|| view! {
                    {move || {
                        let fails = c.overview.get().map(|o| o.o.health.iter().filter(|h| h.status == sl_proto::panel::CheckStatus::Fail).count()).unwrap_or(0);
                        (fails > 0).then(|| view! { <span class="tag fail" aria-label=format!("{fails} problems")>{fails}</span> })
                    }}
                })}
                {(p == Page::Announcements).then(|| view! {
                    {move || {
                        let n = c.overview.get().map(|o| o.o.active_broadcasts).unwrap_or(0);
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
            <nav class="nav" aria-label="Control panel">
                <div class="brand"><Mark/><span>"SpareLine"<small>"Server control panel"</small></span></div>
                {btn(Page::Overview, "home", "Overview")}
                {btn(Page::People, "people", "People")}
                {btn(Page::Groups, "groups", "Groups")}
                {btn(Page::Invites, "invite", "Invites")}
                {btn(Page::Announcements, "megaphone", "Announcements")}
                {btn(Page::Activity, "pulse", "Activity")}
                {btn(Page::Diagnostics, "tool", "Diagnostics")}
                {btn(Page::Settings, "settings", "Settings")}
                <div class="nav-foot">
                    <div class="row">
                        <span class="grow small label-text truncate">{move || c.overview.get().map(|o| o.o.server_name).unwrap_or_default()}</span>
                        <AaButton align_left=true/>
                    </div>
                    <button class="btn quiet small" on:click=lock><Icon name="lock"/><span class="label-text">"Lock panel"</span></button>
                </div>
            </nav>
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
