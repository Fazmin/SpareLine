//! Overview: is the server up, can people reach it, and is anything wrong?
//!
//! The data refreshes every 2 seconds; sections are memoised so only what changed is
//! redrawn (charts keep their hover state, buttons keep focus).

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::panel::{CheckStatus, HealthCheck, ServerStatus, StatPoint, SystemStats};
use sl_ui::{Icon, format, ipc, toasts};

use crate::chart::TimeChart;
use crate::ctx::{Page, call, ctx};
use crate::shell::poll_overview;

pub fn status_tag(s: CheckStatus) -> impl IntoView {
    let (class, icon, label) = match s {
        CheckStatus::Ok => ("tag ok", "check", "OK"),
        CheckStatus::Warn => ("tag warn", "alert", "Check"),
        CheckStatus::Fail => ("tag fail", "alert", "Problem"),
        CheckStatus::Unknown => ("tag", "info", "Unknown"),
    };
    view! { <span class=class><Icon name=icon/>{label}</span> }
}

/// A health-check row with an optional one-click fix.
pub fn check_row(h: HealthCheck, after: Callback<()>) -> impl IntoView {
    let busy = RwSignal::new(false);
    let fix = h.fix.clone();
    view! {
        <li class="check">
            {status_tag(h.status)}
            <div>
                <strong>{h.name.clone()}</strong>
                <span class="detail">{h.detail.clone()}</span>
            </div>
            {fix.map(|f| {
                let id = f.id.clone();
                view! {
                    <button class="btn quiet small" disabled=move || busy.get() on:click=move |_| {
                        busy.set(true);
                        let id = id.clone();
                        spawn_local(async move {
                            #[derive(Serialize)]
                            struct A { id: String }
                            match call::<String>("run_fix", A { id }).await {
                                Ok(m) => toasts().info(m),
                                Err(e) => toasts().error(e.message),
                            }
                            busy.set(false);
                            after.run(());
                        });
                    }>{f.label.clone()}</button>
                }
            })}
        </li>
    }
}

fn meter(used: u64, total: u64) -> impl IntoView {
    let pct = if total == 0 {
        0.0
    } else {
        used as f64 * 100.0 / total as f64
    };
    let level = if pct > 92.0 {
        "fail"
    } else if pct > 80.0 {
        "warn"
    } else {
        "ok"
    };
    view! { <div class="meter" aria-hidden="true"><i data-level=level style=format!("width:{pct:.0}%")></i></div> }
}

#[derive(Clone, PartialEq)]
struct Head {
    status: ServerStatus,
    online: u32,
    addresses: String,
    fingerprint: String,
    started_at: i64,
    start_error: Option<String>,
    debug_until: Option<i64>,
}

#[derive(Clone, PartialEq)]
struct Numbers {
    online: u32,
    total: u32,
    ready: u32,
    devices: u32,
    hour: u64,
    day: u64,
}

#[component]
pub fn OverviewPage() -> impl IntoView {
    let c = ctx();
    let loaded = Memo::new(move |_| c.overview.with(|o| o.is_some()));
    let head = Memo::new(move |_| {
        c.overview.with(|o| {
            o.as_ref().map(|d| Head {
                status: d.o.status,
                online: d.o.users_online,
                addresses: d.o.addresses.join(", "),
                fingerprint: d.o.fingerprint.clone(),
                started_at: d.o.started_at,
                start_error: d.start_error.clone(),
                debug_until: d.o.debug_mode_until,
            })
        })
    });
    let numbers = Memo::new(move |_| {
        c.overview.with(|o| {
            o.as_ref().map(|d| Numbers {
                online: d.o.users_online,
                total: d.o.users_total,
                ready: d.o.devices_ready,
                devices: d.o.devices_total,
                hour: d.o.messages_last_hour,
                day: d.o.messages_last_24h,
            })
        })
    });
    let series = Memo::new(move |_| {
        c.overview.with(|o| {
            o.as_ref()
                .map(|d| {
                    let s = &d.o.series;
                    s[s.len() - s.len().min(60)..].to_vec()
                })
                .unwrap_or_default()
        })
    });
    let problems = Memo::new(move |_| {
        c.overview.with(|o| {
            o.as_ref()
                .map(|d| {
                    d.o.health
                        .iter()
                        .filter(|h| matches!(h.status, CheckStatus::Fail | CheckStatus::Warn))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let check_count = Memo::new(move |_| {
        c.overview
            .with(|o| o.as_ref().map(|d| d.o.health.len()).unwrap_or(0))
    });
    let sys = Memo::new(move |_| {
        c.overview
            .with(|o| o.as_ref().map(|d| (d.o.system.clone(), d.o.cert_not_after)))
    });

    let copy = move |text: String| {
        spawn_local(async move {
            if ipc::copy_text(&text).await {
                toasts().info("Copied.");
            }
        });
    };
    let debug_off = move |_| {
        #[derive(Serialize)]
        struct A {
            minutes: u32,
        }
        crate::ctx::act(
            "set_debug_mode",
            A { minutes: 0 },
            "Debug logging is off.",
            poll_overview,
        );
    };
    let pick = move |f: fn(&StatPoint) -> u32| {
        move || {
            series
                .get()
                .iter()
                .map(|p| (p.ts, f(p)))
                .collect::<Vec<_>>()
        }
    };

    let header = move || {
        head.get().map(|h| {
            let state = match h.status {
                ServerStatus::Running => "running",
                ServerStatus::Degraded => "degraded",
                ServerStatus::Stopped => "stopped",
            };
            let headline = match h.status {
                ServerStatus::Running => match h.online {
                    0 => "SpareLine is running. Nobody is connected right now.".to_string(),
                    1 => "SpareLine is running. 1 person is connected.".to_string(),
                    n => format!("SpareLine is running. {n} people are connected."),
                },
                ServerStatus::Degraded => "SpareLine is running, but something needs attention.".to_string(),
                ServerStatus::Stopped => "SpareLine is stopped. Staff can't connect.".to_string(),
            };
            let addrs = h.addresses.clone();
            view! {
                <div class="status-head">
                    <span class="status-lamp" data-s=state aria-hidden="true"></span>
                    <div>
                        <h1>{headline}</h1>
                        <div class="reach">
                            <span>"Staff connect to "<span class="mono">{if h.addresses.is_empty() { "(no network address)".to_string() } else { h.addresses.clone() }}</span></span>
                            <button class="btn ghost small" on:click=move |_| copy(addrs.clone())><Icon name="copy"/>"Copy address"</button>
                        </div>
                        <p class="small muted" style="margin-top:var(--s2)">
                            "Server fingerprint "<span class="mono">{h.fingerprint.clone()}</span>
                            {format!(". Running for {}.", format::duration(format::now() - h.started_at))}
                        </p>
                    </div>
                </div>
                {h.start_error.clone().map(|e| view! {
                    <div class="notice fail" role="alert" style="margin-top:var(--s4)"><Icon name="alert"/><div><strong>"The server couldn't start"</strong><p class="small">{e}</p></div></div>
                })}
                {h.debug_until.map(|t| view! {
                    <div class="notice warn" role="status" style="margin-top:var(--s4)">
                        <Icon name="bug"/>
                        <span class="grow">{format!("Debug logging is on until {}. Logs are more detailed but never include messages.", format::time(t))}</span>
                        <button class="btn quiet small" on:click=debug_off>"Turn off"</button>
                    </div>
                })}
            }
        })
    };

    let strip =
        move || {
            numbers.get().map(|n| view! {
            <div class="strip" role="group" aria-label="Key numbers">
                <div>
                    <div class="n">{n.online}<small>{format!(" of {}", n.total)}</small></div>
                    <div class="l">"People online"</div>
                </div>
                <div title="Devices that connected at least once in the last 7 days">
                    <div class="n">{n.ready}<small>{format!(" of {}", n.devices)}</small></div>
                    <div class="l">"Devices ready (seen this week)"</div>
                </div>
                <div>
                    <div class="n">{n.hour}</div>
                    <div class="l">"Messages relayed, last hour"</div>
                </div>
                <div>
                    <div class="n">{n.day}</div>
                    <div class="l">"Messages relayed, last 24 hours"</div>
                </div>
            </div>
        })
        };

    let system = move || {
        sys.get().map(|(s, cert): (SystemStats, i64)| view! {
            <dl class="sys">
                <div><dt>"Processor"</dt><dd>{format!("{:.0}%", s.cpu_percent)}</dd>{meter((s.cpu_percent * 10.0) as u64, 1000)}</div>
                <div><dt>"Memory"</dt><dd>{format!("{} of {}", format::bytes(s.mem_used), format::bytes(s.mem_total))}</dd>{meter(s.mem_used, s.mem_total)}</div>
                <div><dt>"Disk free"</dt><dd>{format::bytes(s.disk_free)}</dd>{meter(s.disk_total.saturating_sub(s.disk_free), s.disk_total)}</div>
                <div><dt>"SpareLine memory"</dt><dd>{format::bytes(s.process_mem)}</dd></div>
                <div><dt>"Database"</dt><dd>{format::bytes(s.db_size)}</dd></div>
                <div><dt>"Certificate valid until"</dt><dd>{format::full(cert)}</dd></div>
            </dl>
        })
    };

    view! {
        <Show when=move || loaded.get() fallback=|| view! { <div class="empty"><sl_ui::Spinner label="Loading"/></div> }>
            {header}
            {strip}
            <div class="charts">
                <TimeChart title="Messages relayed per minute" points=Signal::derive(pick(|p| p.messages)) one="message" many="messages" columns=true/>
                <TimeChart title="Connected devices" points=Signal::derive(pick(|p| p.connections)) one="device" many="devices" columns=false/>
            </div>
            <section class="section" aria-labelledby="h-health">
                <div class="section-title">
                    <h2 id="h-health">"Health"</h2>
                    <button class="btn ghost small" on:click=move |_| c.page.set(Page::Diagnostics)>"Open diagnostics"</button>
                </div>
                {move || {
                    let p = problems.get();
                    if p.is_empty() {
                        view! { <div class="notice ok"><Icon name="check"/><span>{format!("All {} checks look good.", check_count.get())}</span></div> }.into_any()
                    } else {
                        view! { <div class="box flush"><ul class="checks">{p.into_iter().map(|h| check_row(h, Callback::new(|_| poll_overview()))).collect_view()}</ul></div> }.into_any()
                    }
                }}
            </section>
            <section class="section" aria-labelledby="h-sys">
                <div class="section-title"><h2 id="h-sys">"This computer"</h2></div>
                <div class="box">{system}</div>
            </section>
            <p class="privacy-note"><Icon name="shield"/><span>"Conversations are end-to-end encrypted. Their content never reaches this server, so it can't be shown here or anywhere else."</span></p>
        </Show>
    }
}
