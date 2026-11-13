//! Activity: who is connected, connection problems, and the live server log.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::panel::*;
use sl_ui::{Icon, format};

use crate::ctx::{Page, call, call0, ctx};
use crate::people::event_tag;
use crate::shell::poll_overview;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Now,
    Problems,
    Log,
}

#[component]
pub fn ActivityPage() -> impl IntoView {
    let tab = RwSignal::new(Tab::Now);
    let t = move |v: Tab, label: &'static str| {
        view! { <label><input type="radio" name="activity-tab" prop:checked=move || tab.get() == v on:change=move |_| tab.set(v)/>{label}</label> }
    };
    view! {
        <div class="page-head">
            <div class="grow">
                <h1>"Activity"</h1>
                <p class="lede">"Live connections, connection problems and the server log. None of this includes message content."</p>
            </div>
        </div>
        <div class="seg" role="radiogroup" aria-label="View" style="margin-bottom:var(--s4)">
            {t(Tab::Now, "Connected now")}
            {t(Tab::Problems, "Connection events")}
            {t(Tab::Log, "Server log")}
        </div>
        {move || match tab.get() {
            Tab::Now => view! { <Connected/> }.into_any(),
            Tab::Problems => view! { <Events/> }.into_any(),
            Tab::Log => view! { <Log/> }.into_any(),
        }}
    }
}

#[component]
fn Connected() -> impl IntoView {
    let c = ctx();
    let rows = RwSignal::new(None::<Vec<ConnectionRow>>);
    let load = move || {
        spawn_local(async move {
            if let Ok(r) = call0::<Vec<ConnectionRow>>("connections").await
                && rows.get_untracked().as_ref() != Some(&r)
            {
                rows.set(Some(r));
            }
        });
    };
    load();
    if let Ok(h) = set_interval_with_handle(load, std::time::Duration::from_secs(2)) {
        on_cleanup(move || h.clear());
    }
    view! {
        <div class="box flush">
            {move || match rows.get() {
                None => view! { <div class="empty"><sl_ui::Spinner/></div> }.into_any(),
                Some(r) if r.is_empty() => view! { <div class="empty"><h2>"Nobody is connected"</h2><p>"Devices appear here as soon as they connect."</p></div> }.into_any(),
                Some(r) => view! {
                    <table class="table">
                        <thead><tr><th scope="col">"Person"</th><th scope="col">"Device"</th><th scope="col">"Address"</th><th scope="col">"Connected"</th><th scope="col">"Latency"</th></tr></thead>
                        <tbody>
                            {r.into_iter().map(|x| {
                                let uid = x.user.id;
                                view! {
                                    <tr class="clickable" on:click=move |_| { c.focus_user.set(Some(uid)); c.page.set(Page::People); }>
                                        <td><strong>{x.user.display_name.clone()}</strong><span class="hint" style="display:block">{format!("@{}", x.user.username)}</span></td>
                                        <td>{x.device_name.clone()}<span class="hint" style="display:block">{format!("{} {}", x.platform, x.client_version)}</span></td>
                                        <td class="mono">{x.ip.clone()}</td>
                                        <td class="hint">{format::ago(x.connected_at)}</td>
                                        <td>{x.rtt_ms.map(|ms| format!("{ms} ms")).unwrap_or_else(|| "measuring".into())}</td>
                                    </tr>
                                }
                            }).collect_view()}
                        </tbody>
                    </table>
                }.into_any(),
            }}
        </div>
    }
}

#[component]
fn Events() -> impl IntoView {
    let rows = RwSignal::new(None::<Vec<ConnectionEvent>>);
    let problems_only = RwSignal::new(true);
    let q = RwSignal::new(String::new());
    let load = move || {
        spawn_local(async move {
            #[derive(Serialize)]
            struct A {
                user: Option<uuid::Uuid>,
                limit: u32,
            }
            if let Ok(r) = call::<Vec<ConnectionEvent>>(
                "connection_events",
                A {
                    user: None,
                    limit: 500,
                },
            )
            .await
                && rows.get_untracked().as_ref() != Some(&r)
            {
                rows.set(Some(r));
            }
        });
    };
    load();
    if let Ok(h) = set_interval_with_handle(load, std::time::Duration::from_secs(4)) {
        on_cleanup(move || h.clear());
    }
    let shown = move || {
        let query = q.get().to_lowercase();
        rows.get()
            .unwrap_or_default()
            .into_iter()
            .filter(|e| {
                (!problems_only.get()
                    || matches!(
                        e.kind,
                        ConnEventKind::TlsFailed
                            | ConnEventKind::AuthFailed
                            | ConnEventKind::ProtocolMismatch
                            | ConnEventKind::RateLimited
                    ))
                    && (query.is_empty()
                        || e.ip.contains(&query)
                        || e.detail.to_lowercase().contains(&query)
                        || e.user.as_ref().is_some_and(|u| {
                            u.display_name.to_lowercase().contains(&query)
                                || u.username.contains(&query)
                        }))
            })
            .take(300)
            .collect::<Vec<_>>()
    };
    view! {
        <div class="toolbar">
            <input class="input" type="search" aria-label="Filter events" placeholder="Filter by person, address or reason" prop:value=move || q.get() on:input=move |e| q.set(event_target_value(&e))/>
            <label class="row" style="cursor:pointer">
                <input type="checkbox" style="width:1.1rem;height:1.1rem;accent-color:var(--signal)" prop:checked=move || problems_only.get() on:change=move |e| problems_only.set(event_target_checked(&e))/>
                "Problems only"
            </label>
        </div>
        <div class="box flush">
            {move || {
                let r = shown();
                if rows.get().is_none() {
                    view! { <div class="empty"><sl_ui::Spinner/></div> }.into_any()
                } else if r.is_empty() {
                    view! { <div class="empty"><h2>"Nothing to show"</h2><p>"No connection problems recorded in the last week."</p></div> }.into_any()
                } else {
                    view! {
                        <table class="table">
                            <thead><tr><th scope="col">"When"</th><th scope="col">"What"</th><th scope="col">"Who"</th><th scope="col">"Why"</th></tr></thead>
                            <tbody>
                                {r.into_iter().map(|e| view! {
                                    <tr>
                                        <td class="hint" style="white-space:nowrap">{format::full(e.ts)}</td>
                                        <td>{event_tag(e.kind)}</td>
                                        <td>{e.user.map(|u| u.display_name).unwrap_or_else(|| "Unknown".into())}<span class="hint mono" style="display:block">{e.ip.clone()}</span></td>
                                        <td class="small">{e.detail.clone()}</td>
                                    </tr>
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_any()
                }
            }}
        </div>
    }
}

fn level_str(l: LogLevel) -> &'static str {
    match l {
        LogLevel::Trace => "trace",
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

#[component]
fn Log() -> impl IntoView {
    let c = ctx();
    let lines = RwSignal::new(Vec::<LogEntry>::new());
    let min = RwSignal::new(LogLevel::Info);
    let text = RwSignal::new(String::new());
    let paused = RwSignal::new(false);
    let el: NodeRef<leptos::html::Div> = NodeRef::new();
    let fetch = move |reset: bool| {
        if paused.get_untracked() && !reset {
            return;
        }
        let after = if reset {
            None
        } else {
            lines.with_untracked(|l| l.last().map(|e| e.seq))
        };
        let q = LogQuery {
            min_level: Some(min.get_untracked()),
            target: None,
            text: Some(text.get_untracked()).filter(|t| !t.is_empty()),
            after_seq: after,
            limit: Some(1000),
        };
        spawn_local(async move {
            #[derive(Serialize)]
            struct A {
                query: LogQuery,
            }
            if let Ok(mut r) = call::<Vec<LogEntry>>("logs", A { query: q }).await {
                if reset {
                    lines.set(r);
                } else if !r.is_empty() {
                    lines.update(|l| {
                        l.append(&mut r);
                        let n = l.len();
                        if n > 3000 {
                            l.drain(..n - 3000);
                        }
                    });
                } else {
                    return;
                }
                request_animation_frame(move || {
                    if let Some(d) = el.get_untracked() {
                        d.set_scroll_top(d.scroll_height());
                    }
                });
            }
        });
    };
    fetch(true);
    if let Ok(h) = set_interval_with_handle(move || fetch(false), std::time::Duration::from_secs(1))
    {
        on_cleanup(move || h.clear());
    }
    let debug_on = move || c.overview.get().and_then(|o| o.o.debug_mode_until);
    let set_debug = move |minutes: u32| {
        #[derive(Serialize)]
        struct A {
            minutes: u32,
        }
        crate::ctx::act(
            "set_debug_mode",
            A { minutes },
            if minutes > 0 {
                "Debug logging is on."
            } else {
                "Debug logging is off."
            },
            poll_overview,
        );
    };
    view! {
        <div class="toolbar">
            <select class="select" aria-label="Minimum level" on:change=move |e| {
                min.set(match event_target_value(&e).as_str() { "trace" => LogLevel::Trace, "debug" => LogLevel::Debug, "warn" => LogLevel::Warn, "error" => LogLevel::Error, _ => LogLevel::Info });
                fetch(true);
            }>
                <option value="error">"Errors"</option>
                <option value="warn">"Warnings and errors"</option>
                <option value="info" selected>"Normal"</option>
                <option value="debug">"Detailed"</option>
                <option value="trace">"Everything"</option>
            </select>
            <input class="input" type="search" aria-label="Search the log" placeholder="Search the log" prop:value=move || text.get() on:input=move |e| { text.set(event_target_value(&e)); fetch(true); }/>
            <button class="btn quiet small" aria-pressed=move || paused.get().to_string() on:click=move |_| paused.update(|p| *p = !*p)>
                {move || if paused.get() { "Resume" } else { "Pause" }}
            </button>
            <span class="grow"></span>
            {move || match debug_on() {
                Some(t) => view! {
                    <span class="tag warn"><Icon name="bug"/>{format!("Debug logging until {}", format::time(t))}</span>
                    <button class="btn quiet small" on:click=move |_| set_debug(0)>"Turn off"</button>
                }.into_any(),
                None => view! {
                    <button class="btn quiet small" on:click=move |_| set_debug(30) title="More detail about connections, sign-ins and discovery for 30 minutes"><Icon name="bug"/>"Debug logging for 30 min"</button>
                }.into_any(),
            }}
        </div>
        <div class="log-view" node_ref=el role="log" aria-label="Server log" tabindex="0">
            <For each=move || lines.get() key=|l| l.seq let:l>
                <div class="log-line" data-l=level_str(l.level)>
                    <span class="hint">{format::time(l.ts)}</span>
                    <span class="lvl">{level_str(l.level).to_uppercase()}</span>
                    <span class="hint truncate" title=l.target.clone()>{l.target.trim_start_matches("sl_server::").to_string()}</span>
                    <span class="msg">{l.message.clone()}" "<span class="fields">{l.fields.clone()}</span></span>
                </div>
            </For>
        </div>
        <p class="hint" style="margin-top:var(--s2)">"Logs are also written to the logs folder in the server's data folder, kept for 14 days."</p>
    }
}
