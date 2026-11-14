//! Diagnostics: health checks with fixes, an end-to-end self-test, reports, audit log.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::panel::*;
use sl_ui::{Icon, format, toasts};

use crate::ctx::{call, call0, ctx};
use crate::overview::{check_row, status_tag};
use crate::shell::poll_overview;

#[component]
pub fn DiagnosticsPage() -> impl IntoView {
    let c = ctx();
    let checks = RwSignal::new(None::<Vec<HealthCheck>>);
    let running = RwSignal::new(false);
    let steps = RwSignal::new(None::<Vec<SelfTestStep>>);
    let testing = RwSignal::new(false);
    let audit = RwSignal::new(Vec::<AuditRow>::new());
    let run = move || {
        running.set(true);
        spawn_local(async move {
            if let Ok(h) = call0::<Vec<HealthCheck>>("health").await {
                checks.set(Some(h));
            }
            running.set(false);
            poll_overview();
        });
    };
    run();
    spawn_local(async move {
        #[derive(Serialize)]
        struct A {
            limit: u32,
        }
        if let Ok(a) = call::<Vec<AuditRow>>("audit", A { limit: 100 }).await {
            audit.set(a);
        }
    });
    let self_test = move |_| {
        testing.set(true);
        steps.set(None);
        spawn_local(async move {
            if let Ok(s) = call0::<Vec<SelfTestStep>>("self_test").await {
                steps.set(Some(s));
            }
            testing.set(false);
        });
    };
    let report = move |_| {
        spawn_local(async move {
            match call0::<String>("diagnostics_report").await {
                Ok(p) => toasts().info(format!("Report saved: {p}")),
                Err(e) => toasts().error(e.message),
            }
        });
    };
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
    let debug_until = move || c.overview.get().and_then(|o| o.o.debug_mode_until);
    view! {
        <div class="page-head">
            <div class="grow">
                <h1>"Diagnostics"</h1>
                <p class="lede">"Check that staff can reach and use this server. Start here when someone says SpareLine isn't connecting."</p>
            </div>
            <button class="btn quiet" disabled=move || running.get() on:click=move |_| run()><Icon name="refresh"/>"Run checks again"</button>
        </div>
        <section aria-labelledby="h-checks">
            <div class="section-title"><h2 id="h-checks">"Health checks"</h2></div>
            {move || checks.get().map(|h| {
                let fails = h.iter().filter(|x| x.status == CheckStatus::Fail).count();
                let warns = h.iter().filter(|x| x.status == CheckStatus::Warn).count();
                let passed = h.iter().filter(|x| x.status == CheckStatus::Ok).count();
                let tone = if fails > 0 { "fail" } else { "warn" };
                let (state, icon, title) = match fails + warns {
                    0 => ("ok", "check", "All checks passed".to_string()),
                    1 => (tone, "alert", "1 check needs you".to_string()),
                    n => (tone, "alert", format!("{n} checks need you")),
                };
                view! {
                    <div class="health-sign" data-s=state role="status">
                        <Icon name=icon/>
                        <div><strong>{title}</strong><span>{format!("{passed} of {} passed.", h.len())}</span></div>
                    </div>
                }
            })}
            <div class="box flush">
                {move || match checks.get() {
                    None => view! { <div class="empty"><sl_ui::Spinner label="Running checks"/></div> }.into_any(),
                    Some(h) => view! { <ul class="checks">{h.into_iter().map(|x| check_row(x, Callback::new(move |_| run()))).collect_view()}</ul> }.into_any(),
                }}
            </div>
        </section>
        <section class="section" aria-labelledby="h-self">
            <div class="section-title">
                <h2 id="h-self">"Connection test"</h2>
                <button class="btn" disabled=move || testing.get() on:click=self_test>
                    {move || if testing.get() { view! { <sl_ui::Spinner/> "Testing…" }.into_any() } else { view! { <Icon name="pulse"/> "Test like a staff computer would" }.into_any() }}
                </button>
            </div>
            <div class="box">
                <p class="muted">"Connects to this server over its network address, checks the secure connection and its fingerprint, opens a chat channel, and confirms the server can be found automatically."</p>
                {move || steps.get().map(|s| view! {
                    <ul class="checks" style="margin-top:var(--s3)">
                        {s.into_iter().map(|st| view! {
                            <li class="check" style="padding-left:0;padding-right:0">
                                {status_tag(st.status)}
                                <div><strong>{st.name.clone()}</strong><span class="detail">{st.detail.clone()}</span></div>
                                <span class="hint">{format!("{} ms", st.millis)}</span>
                            </li>
                        }).collect_view()}
                    </ul>
                })}
            </div>
        </section>
        <section class="section" aria-labelledby="h-debug">
            <div class="section-title"><h2 id="h-debug">"Debug logging"</h2></div>
            <div class="box stack">
                <p class="muted">"Records more detail about connections, sign-ins, encryption bookkeeping and network discovery. It switches itself off. Messages are never logged: the server can't read them."</p>
                {move || match debug_until() {
                    Some(t) => view! {
                        <div class="row"><span class="tag warn"><Icon name="bug"/>{format!("On until {}", format::time(t))}</span><button class="btn quiet small" on:click=move |_| set_debug(0)>"Turn off now"</button></div>
                    }.into_any(),
                    None => view! {
                        <div class="row wrap">
                            <button class="btn quiet small" on:click=move |_| set_debug(15)>"On for 15 minutes"</button>
                            <button class="btn quiet small" on:click=move |_| set_debug(30)>"On for 30 minutes"</button>
                            <button class="btn quiet small" on:click=move |_| set_debug(60)>"On for 1 hour"</button>
                        </div>
                    }.into_any(),
                }}
            </div>
        </section>
        <section class="section" aria-labelledby="h-report">
            <div class="section-title"><h2 id="h-report">"Report for support"</h2></div>
            <div class="box row wrap">
                <p class="grow muted">"Saves health checks, recent connection events, settings and the recent log to a text file you can send to whoever supports this server."</p>
                <button class="btn quiet" on:click=report><Icon name="download"/>"Create report"</button>
            </div>
        </section>
        <section class="section" aria-labelledby="h-audit">
            <div class="section-title"><h2 id="h-audit">"Admin activity"</h2></div>
            <div class="box flush">
                <table class="table">
                    <thead><tr><th scope="col">"When"</th><th scope="col">"Action"</th><th scope="col">"Subject"</th><th scope="col">"Detail"</th></tr></thead>
                    <tbody>
                        {move || audit.get().into_iter().map(|a| view! {
                            <tr><td class="hint" style="white-space:nowrap">{format::full(a.ts)}</td><td class="mono small">{a.action}</td><td>{a.target}</td><td class="small">{a.detail}</td></tr>
                        }).collect_view()}
                    </tbody>
                </table>
            </div>
        </section>
    }
}
