//! Panel layout: the sidebar, the bar above each page, the command menu, page routing and
//! overview polling.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::panel::{CheckStatus, ServerStatus};
use sl_ui::prefs::ThemePref;
use sl_ui::{
    AaButton, Command, CommandMenu, Icon, Mark, PrefsCtx, SignalLine, ipc, mod_key, toasts,
};
use wasm_bindgen::JsCast;

use crate::ctx::{Ctx, OverviewData, Page, call0, ctx};

pub fn poll_overview() {
    let c = ctx();
    spawn_local(async move {
        if let Ok(o) = call0::<OverviewData>("overview").await {
            c.overview.set(Some(o));
        }
    });
}

/// How the server's state is named and drawn: `(data-s value, icon, words)`.
/// The sidebar status and the overview header both use it, so the state is never colour alone.
pub fn status_look(s: ServerStatus) -> (&'static str, &'static str, &'static str) {
    match s {
        ServerStatus::Running => ("running", "check", "Running"),
        ServerStatus::Degraded => ("degraded", "alert", "Running with problems"),
        ServerStatus::Stopped => ("stopped", "close", "Stopped"),
    }
}

/// Every page in sidebar order: `(page, icon, name, the key after G)`.
pub const PAGES: [(Page, &str, &str, &str); 8] = [
    (Page::Overview, "home", "Overview", "O"),
    (Page::People, "people", "People", "P"),
    (Page::Invites, "invite", "Invites", "I"),
    (Page::Activity, "pulse", "Activity", "A"),
    (Page::Diagnostics, "tool", "Diagnostics", "D"),
    (Page::Groups, "groups", "Groups", "G"),
    (Page::Announcements, "megaphone", "Announcements", "N"),
    (Page::Settings, "settings", "Settings", "S"),
];

fn page_info(p: Page) -> (&'static str, &'static str) {
    PAGES
        .iter()
        .find(|x| x.0 == p)
        .map(|x| (x.1, x.2))
        .unwrap_or(("home", "Overview"))
}

fn lock_panel(c: Ctx) {
    spawn_local(async move {
        #[derive(Serialize)]
        struct No {}
        let _ = ipc::invoke::<()>("lock", No {}).await;
        c.locked();
    });
}

fn copy_address(c: Ctx) {
    let text = c
        .overview
        .with_untracked(|o| o.as_ref().map(|d| d.o.addresses.join(", ")))
        .unwrap_or_default();
    spawn_local(async move {
        if text.is_empty() {
            toasts().warn("This computer has no network address yet.");
        } else if ipc::copy_text(&text).await {
            toasts().info("Address copied.");
        }
    });
}

/// True when a key press is going into a text field, so single-key shortcuts stay out of the way.
fn typing(e: &leptos::ev::KeyboardEvent) -> bool {
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
        .is_some_and(|el| {
            matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                || el.is_content_editable()
        })
}

#[component]
pub fn Shell() -> impl IntoView {
    let c = ctx();
    let PrefsCtx(prefs) = expect_context::<PrefsCtx>();
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
    let people = Memo::new(move |_| c.overview.with(|o| o.as_ref().map(|d| d.o.users_total)));
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

    // Command menu and keyboard shortcuts: Cmd/Ctrl+K opens the menu, G then a letter
    // jumps to a page (shown next to each page in the menu).
    let menu_open = RwSignal::new(false);
    let query = RwSignal::new(String::new());
    let g_pressed = StoredValue::new(0.0_f64);
    let keys = window_event_listener(leptos::ev::keydown, move |e| {
        let k = e.key();
        if (e.ctrl_key() || e.meta_key()) && k.eq_ignore_ascii_case("k") {
            e.prevent_default();
            menu_open.update(|o| *o = !*o);
            return;
        }
        if e.ctrl_key() || e.meta_key() || e.alt_key() || menu_open.get_untracked() || typing(&e) {
            return;
        }
        let now = js_sys::Date::now();
        if now - g_pressed.get_value() < 1500.0 {
            g_pressed.set_value(0.0);
            if let Some(p) = PAGES.iter().find(|x| x.3.eq_ignore_ascii_case(&k)) {
                e.prevent_default();
                c.page.set(p.0);
            }
        } else if k.eq_ignore_ascii_case("g") {
            g_pressed.set_value(now);
        }
    });
    on_cleanup(move || keys.remove());

    let commands = Signal::derive(move || {
        let mut v: Vec<Command> = PAGES
            .iter()
            .map(|&(p, icon, name, key)| {
                Command::new("Go to", icon, name, move || c.page.set(p)).keys(&["G", key])
            })
            .collect();
        v.push(Command::new(
            "Actions",
            "plus",
            "Create invites",
            move || {
                c.invite_now.set(true);
                c.page.set(Page::Invites);
            },
        ));
        v.push(Command::new(
            "Actions",
            "megaphone",
            "Write an announcement",
            move || c.page.set(Page::Announcements),
        ));
        v.push(Command::new(
            "Actions",
            "refresh",
            "Run health checks",
            move || c.page.set(Page::Diagnostics),
        ));
        v.push(Command::new(
            "Actions",
            "copy",
            "Copy the staff address",
            move || copy_address(c),
        ));
        v.push(Command::new(
            "Actions",
            "lock",
            "Lock the panel",
            move || lock_panel(c),
        ));
        let theme = prefs.with(|p| p.theme);
        for (t, label) in [
            (ThemePref::System, "Match the system theme"),
            (ThemePref::Light, "Use the light theme"),
            (ThemePref::Dark, "Use the dark theme"),
        ] {
            let mut cmd = Command::new("Appearance", "eye", label, move || {
                prefs.update(|p| p.theme = t)
            });
            if t == theme {
                cmd = cmd.trail("Current");
            }
            v.push(cmd);
        }
        let hc = prefs.with(|p| p.high_contrast);
        v.push(Command::new(
            "Appearance",
            "eye",
            if hc {
                "Turn off high contrast"
            } else {
                "Turn on high contrast"
            },
            move || prefs.update(|p| p.high_contrast = !hc),
        ));
        v.push(Command::new(
            "Appearance",
            "plus",
            "Make text bigger",
            move || prefs.update(|p| p.bigger()),
        ));
        v.push(Command::new(
            "Appearance",
            "eye",
            "Make text smaller",
            move || prefs.update(|p| p.smaller()),
        ));

        let q = query.get().to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        v.retain(|cmd| {
            let hay = format!("{} {}", cmd.label, cmd.group).to_lowercase();
            terms.iter().all(|t| hay.contains(t))
        });
        v
    });
    let empty =
        Signal::derive(move || format!("Nothing matches \u{201c}{}\u{201d}.", query.get().trim()));

    let nav_item = move |p: Page, icon: &'static str, label: &'static str| {
        view! {
            <button class="nav-item" title=label aria-current=move || (c.page.get() == p).then_some("page") on:click=move |_| c.page.set(p)>
                <Icon name=icon/>
                <span class="grow truncate">{label}</span>
                {(p == Page::People).then(|| view! {
                    {move || people.get().map(|n| view! { <span class="nav-count" aria-label=format!("{n} people")>{n}</span> })}
                })}
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
    let crumb = Memo::new(move |_| page_info(c.page.get()));
    let open_menu = move |_| menu_open.set(true);

    view! {
        <SignalLine state=line_state label=line_label/>
        <a class="skip-link" href="#main">"Skip to content"</a>
        <div class="panel-app frame">
            <nav class="side" aria-label="Control panel">
                <div class="side-head">
                    <Mark/>
                    <span class="grow">
                        <span class="side-title truncate">"SpareLine Server"</span>
                        <span class="side-sub truncate">{move || server_name.get()}</span>
                    </span>
                </div>
                <button class="side-search" title="Search or jump to" on:click=open_menu aria-keyshortcuts="Control+K Meta+K">
                    <Icon name="search"/>
                    <span class="grow">"Search or jump to…"</span>
                    <span class="kbd" aria-hidden="true">{format!("{}K", mod_key())}</span>
                </button>
                {move || chip.get().map(|(s, online)| {
                    let (state, icon, words) = status_look(s);
                    view! {
                        <button class="side-status" data-s=state title="Open the overview" on:click=move |_| c.page.set(Page::Overview)>
                            <span class="status-icon"><Icon name=icon/></span>
                            <span class="grow truncate">{words}</span>
                            {(s != ServerStatus::Stopped).then(|| view! { <span class="nav-count">{format!("{online} online")}</span> })}
                        </button>
                    }
                })}
                <div class="side-nav">
                    {PAGES.iter().map(|&(p, icon, label, _)| nav_item(p, icon, label)).collect_view()}
                </div>
                <span class="grow"></span>
                <div class="side-foot">
                    <button class="nav-item grow" title="Lock panel" on:click=move |_| lock_panel(c)><Icon name="lock"/><span class="grow">"Lock panel"</span></button>
                    <AaButton align_left=true/>
                </div>
            </nav>
            <div class="view inset">
                <header class="viewbar">
                    <span class="crumb-root truncate">{move || server_name.get()}</span>
                    <Icon name="chevron-right"/>
                    <span class="crumb">
                        {move || { let (icon, _) = crumb.get(); view! { <Icon name=icon/> } }}
                        <span>{move || crumb.get().1}</span>
                    </span>
                </header>
                <main class="main scroll" id="main" tabindex="-1">
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
        </div>
        <CommandMenu
            open=menu_open
            title="Command menu"
            placeholder="Type a command or a page name"
            query=query
            items=commands
            empty=empty
        />
    }
}
