//! Overview: is the server up, can people reach it, and is anything wrong?
//!
//! The status sign says it in one word; the tiles beside it are live summaries that open
//! each page. The data refreshes every 2 seconds; only the text inside the sign and tiles
//! is redrawn, so buttons keep focus and charts keep their hover state.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::panel::{
    BroadcastRow, CheckStatus, GroupRow, HealthCheck, InviteRow, InviteStatus, ServerStatus,
    StatPoint, SystemStats,
};
use sl_ui::{Icon, format, ipc, toasts};

use crate::chart::TimeChart;
use crate::ctx::{Page, call, call0, ctx};
use crate::shell::{poll_overview, status_look};

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
        <li class="check" data-s={match h.status {
            CheckStatus::Fail => "fail",
            CheckStatus::Warn => "warn",
            _ => "ok",
        }}>
            {status_tag(h.status)}
            <div>
                <strong>{h.name.clone()}</strong>
                <span class="detail">{h.detail.clone()}</span>
            </div>
            {fix.map(|f| {
                let id = f.id.clone();
                view! {
                    <button class="btn small" disabled=move || busy.get() on:click=move |_| {
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

/// A tile beside the status sign: a live summary that opens its page.
/// `tone` is `plain`, `live`, `warn` or `fail`; the words always say the same thing.
fn tile(
    page: Page,
    icon: &'static str,
    name: &'static str,
    tone: Signal<&'static str>,
    big: Signal<String>,
    small: Signal<String>,
) -> impl IntoView {
    let c = ctx();
    view! {
        <button class="tile" data-tone=move || tone.get() on:click=move |_| c.page.set(page)>
            <span class="tile-head"><Icon name=icon/><span>{name}</span></span>
            <span class="tile-big">{move || big.get()}</span>
            <span class="tile-small">{move || small.get()}</span>
        </button>
    }
}

#[derive(Clone, PartialEq)]
struct Head {
    status: ServerStatus,
    online: u32,
    total: u32,
    addresses: String,
    fingerprint: String,
    started_at: i64,
    start_error: Option<String>,
    debug_until: Option<i64>,
    problems: usize,
}

#[derive(Clone, PartialEq)]
struct Numbers {
    online: u32,
    total: u32,
    ready: u32,
    devices: u32,
    hour: u64,
    day: u64,
    fails: usize,
    warns: usize,
    checks: usize,
    first_problem: Option<String>,
    live: u32,
}

/// Counts the overview doesn't carry, fetched once when the page opens.
#[derive(Clone, Default, PartialEq)]
struct Extra {
    invites: Option<usize>,
    groups: Option<Vec<String>>,
    live_title: Option<String>,
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
                total: d.o.users_total,
                addresses: d.o.addresses.join(", "),
                fingerprint: d.o.fingerprint.clone(),
                started_at: d.o.started_at,
                start_error: d.start_error.clone(),
                debug_until: d.o.debug_mode_until,
                problems: d
                    .o
                    .health
                    .iter()
                    .filter(|h| matches!(h.status, CheckStatus::Fail | CheckStatus::Warn))
                    .count(),
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
                fails: d
                    .o
                    .health
                    .iter()
                    .filter(|h| h.status == CheckStatus::Fail)
                    .count(),
                warns: d
                    .o
                    .health
                    .iter()
                    .filter(|h| h.status == CheckStatus::Warn)
                    .count(),
                checks: d.o.health.len(),
                first_problem: d
                    .o
                    .health
                    .iter()
                    .find(|h| matches!(h.status, CheckStatus::Fail | CheckStatus::Warn))
                    .map(|h| h.name.clone()),
                live: d.o.active_broadcasts,
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

    let extra = RwSignal::new(Extra::default());
    spawn_local(async move {
        let invites = call0::<Vec<InviteRow>>("invites").await.ok().map(|v| {
            v.iter()
                .filter(|i| matches!(i.status, InviteStatus::Pending))
                .count()
        });
        let groups = call0::<Vec<GroupRow>>("groups")
            .await
            .ok()
            .map(|v| v.into_iter().map(|g| g.name).collect());
        let live_title = call0::<Vec<BroadcastRow>>("broadcasts")
            .await
            .ok()
            .and_then(|v| v.into_iter().find(|b| b.active).map(|b| b.broadcast.title));
        extra.set(Extra {
            invites,
            groups,
            live_title,
        });
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

    let status = Memo::new(move |_| {
        head.with(|h| h.as_ref().map(|h| h.status))
            .unwrap_or(ServerStatus::Running)
    });
    let sign_line = move || {
        head.with(|h| {
            h.as_ref().map(|h| match (h.status, h.online) {
                (ServerStatus::Stopped, _) => "Staff can't connect or send messages.".to_string(),
                (_, 0) => "Nobody is connected right now.".to_string(),
                (_, n) => format!("{n} of {} people connected", h.total),
            })
        })
    };
    let sign_sub = move || {
        head.with(|h| {
            h.as_ref().map(|h| match h.status {
                ServerStatus::Running => {
                    format!("Up {}.", format::duration(format::now() - h.started_at))
                }
                ServerStatus::Degraded => match h.problems {
                    1 => "1 check needs attention. See Health below.".to_string(),
                    n => format!("{n} checks need attention. See Health below."),
                },
                ServerStatus::Stopped => "See Diagnostics for what went wrong.".to_string(),
            })
        })
    };
    let address = move || {
        head.with(|h| {
            h.as_ref().map(|h| {
                if h.addresses.is_empty() {
                    "(no network address)".to_string()
                } else {
                    h.addresses.clone()
                }
            })
        })
    };
    let fingerprint = move || head.with(|h| h.as_ref().map(|h| h.fingerprint.clone()));

    let sign = move || {
        view! {
            <section class="sign" data-s=move || status_look(status.get()).0 aria-labelledby="h-status">
                <div class="sign-top">
                    <span class="sign-icon" aria-hidden="true">{move || view! { <Icon name=status_look(status.get()).1/> }}</span>
                    <div>
                        <h1 id="h-status">{move || status_look(status.get()).2}</h1>
                        <p class="sign-line">{sign_line}</p>
                        <p class="sign-sub">{sign_sub}</p>
                    </div>
                </div>
                <div class="sign-reach">
                    <span class="sign-label">"Staff connect to"</span>
                    <span class="mono sign-addr">{address}</span>
                    <div class="sign-actions">
                        <button class="btn small sign-btn" on:click=move |_| copy(head.get_untracked().map(|h| h.addresses).unwrap_or_default())>
                            <Icon name="copy"/>"Copy address"
                        </button>
                        <span class="sign-fp">"Fingerprint "<span class="mono">{fingerprint}</span></span>
                    </div>
                </div>
            </section>
        }
    };

    let num = move |f: fn(&Numbers) -> String| {
        Signal::derive(move || numbers.with(|n| n.as_ref().map(f).unwrap_or_default()))
    };
    let tiles = move || {
        view! {
            <nav class="tiles" aria-label="Sections">
                {tile(
                    Page::People, "people", "People",
                    Signal::derive(|| "plain"),
                    num(|n| format!("{} online", n.online)),
                    num(|n| format!("of {} people, {} of {} devices ready", n.total, n.ready, n.devices)),
                )}
                {tile(
                    Page::Invites, "invite", "Invites",
                    Signal::derive(|| "plain"),
                    Signal::derive(move || extra.with(|e| match e.invites {
                        None => "…".to_string(),
                        Some(0) => "None open".to_string(),
                        Some(n) => format!("{n} open"),
                    })),
                    Signal::derive(|| "Hand out codes before you need them".to_string()),
                )}
                {tile(
                    Page::Activity, "pulse", "Activity",
                    Signal::derive(|| "plain"),
                    num(|n| format!("{} messages", n.hour)),
                    num(|n| format!("in the last hour, {} in 24 hours", n.day)),
                )}
                {tile(
                    Page::Diagnostics, "tool", "Diagnostics",
                    Signal::derive(move || numbers.with(|n| match n {
                        Some(n) if n.fails > 0 => "fail",
                        Some(n) if n.warns > 0 => "warn",
                        _ => "plain",
                    })),
                    num(|n| match n.fails + n.warns {
                        0 => "All clear".to_string(),
                        k => format!("{k} to check"),
                    }),
                    num(|n| n.first_problem.clone().unwrap_or_else(|| format!("{} checks passed", n.checks))),
                )}
                {tile(
                    Page::Groups, "groups", "Groups",
                    Signal::derive(|| "plain"),
                    Signal::derive(move || extra.with(|e| match e.groups.as_ref().map(Vec::len) {
                        None => "…".to_string(),
                        Some(0) => "No groups yet".to_string(),
                        Some(1) => "1 group".to_string(),
                        Some(n) => format!("{n} groups"),
                    })),
                    Signal::derive(move || extra.with(|e| match &e.groups {
                        Some(g) if !g.is_empty() => g.join(", "),
                        _ => "Decide who can message whom".to_string(),
                    })),
                )}
                {tile(
                    Page::Announcements, "megaphone", "Announcements",
                    Signal::derive(move || if numbers.with(|n| n.as_ref().is_some_and(|n| n.live > 0)) { "live" } else { "plain" }),
                    num(|n| if n.live > 0 { "Live now".to_string() } else { "None showing".to_string() }),
                    Signal::derive(move || {
                        let live = numbers.with(|n| n.as_ref().is_some_and(|n| n.live > 0));
                        match (live, extra.with(|e| e.live_title.clone())) {
                            (true, Some(t)) => t,
                            (true, None) => "Showing at the top of everyone's chat".to_string(),
                            (false, _) => "Show a banner on everyone's SpareLine".to_string(),
                        }
                    }),
                )}
            </nav>
        }
    };

    let notices = move || {
        head.get().map(|h| {
            view! {
                {h.start_error.clone().map(|e| view! {
                    <div class="notice fail" role="alert"><Icon name="alert"/><div><strong>"The server couldn't start"</strong><p class="small">{e}</p></div></div>
                })}
                {h.debug_until.map(|t| view! {
                    <div class="notice warn" role="status">
                        <Icon name="bug"/>
                        <span class="grow">{format!("Debug logging is on until {}. Logs are more detailed but never include messages.", format::time(t))}</span>
                        <button class="btn quiet small" on:click=debug_off>"Turn off"</button>
                    </div>
                })}
            }
        })
    };

    let system = move || {
        sys.get().map(|(s, cert): (SystemStats, i64)| view! {
            <table class="table kv">
                <tbody>
                    <tr><th scope="row">"Processor"</th><td>{format!("{:.0}%", s.cpu_percent)}</td><td class="meter-cell">{meter((s.cpu_percent * 10.0) as u64, 1000)}</td></tr>
                    <tr><th scope="row">"Memory"</th><td>{format!("{} of {}", format::bytes(s.mem_used), format::bytes(s.mem_total))}</td><td class="meter-cell">{meter(s.mem_used, s.mem_total)}</td></tr>
                    <tr><th scope="row">"Disk free"</th><td>{format::bytes(s.disk_free)}</td><td class="meter-cell">{meter(s.disk_total.saturating_sub(s.disk_free), s.disk_total)}</td></tr>
                    <tr><th scope="row">"SpareLine memory"</th><td colspan="2">{format::bytes(s.process_mem)}</td></tr>
                    <tr><th scope="row">"Database"</th><td colspan="2">{format::bytes(s.db_size)}</td></tr>
                    <tr><th scope="row">"Certificate valid until"</th><td colspan="2">{format::full(cert)}</td></tr>
                </tbody>
            </table>
        })
    };

    view! {
        <Show when=move || loaded.get() fallback=|| view! { <div class="empty"><sl_ui::Spinner label="Loading"/></div> }>
            <div class="ov-top">
                {sign}
                {tiles}
            </div>
            {notices}
            <div class="charts">
                <TimeChart title="Messages relayed per minute" points=Signal::derive(pick(|p| p.messages)) one="message" many="messages" columns=true/>
                <TimeChart title="Connected devices" points=Signal::derive(pick(|p| p.connections)) one="device" many="devices" columns=false/>
            </div>
            <div class="ov-bottom">
                <section aria-labelledby="h-health">
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
                <section aria-labelledby="h-sys">
                    <div class="section-title"><h2 id="h-sys">"This computer"</h2></div>
                    <div class="box flush">{system}</div>
                </section>
            </div>
            <p class="privacy-note"><Icon name="shield"/><span>"Conversations are end-to-end encrypted. Their content never reaches this server, so it can't be shown here or anywhere else."</span></p>
        </Show>
    }
}
