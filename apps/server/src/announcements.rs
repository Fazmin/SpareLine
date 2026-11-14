//! Announcements: one-way banners on every client ("Our usual chat is down, use SpareLine").

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::Severity;
use sl_proto::panel::*;
use sl_ui::{Icon, format, toasts};

use crate::ctx::{act, call, call0};
use crate::shell::poll_overview;

#[component]
pub fn AnnouncementsPage() -> impl IntoView {
    let list = RwSignal::new(None::<Vec<BroadcastRow>>);
    let title = RwSignal::new(String::new());
    let body = RwSignal::new(String::new());
    let sev = RwSignal::new(Severity::Warning);
    let ends = RwSignal::new(0i64);
    let load = move || {
        spawn_local(async move {
            if let Ok(l) = call0::<Vec<BroadcastRow>>("broadcasts").await {
                list.set(Some(l));
            }
        });
    };
    load();
    let preset = move |t: &str, b: &str| {
        title.set(t.into());
        body.set(b.into());
        sev.set(Severity::Warning);
    };
    let send = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        if title.get_untracked().trim().is_empty() {
            toasts().warn("Write a short headline first.");
            return;
        }
        let input = BroadcastInput {
            title: title.get_untracked(),
            body: body.get_untracked(),
            severity: sev.get_untracked(),
            expires_in_secs: (ends.get_untracked() > 0).then_some(ends.get_untracked()),
        };
        #[derive(Serialize)]
        struct A {
            input: BroadcastInput,
        }
        spawn_local(async move {
            match call::<()>("create_broadcast", A { input }).await {
                Ok(()) => {
                    toasts().info("Announcement sent to everyone who's connected. Others see it when they connect.");
                    title.set(String::new());
                    body.set(String::new());
                    load();
                    poll_overview();
                }
                Err(e) => toasts().error(e.message),
            }
        });
    };
    let level = move || match sev.get() {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Critical => "critical",
    };
    let sev_opt = move |s: Severity, label: &'static str| {
        view! { <label><input type="radio" name="sev" prop:checked=move || sev.get() == s on:change=move |_| sev.set(s)/>{label}</label> }
    };
    view! {
        <div class="page-head">
            <span class="page-icon" aria-hidden="true"><Icon name="megaphone"/></span>
            <div class="grow">
                <h1>"Announcements"</h1>
                <p class="lede">"Show a banner at the top of everyone's SpareLine. Use it to say what's happening and where people should talk. Announcements are not encrypted, so don't include anything confidential."</p>
            </div>
        </div>
        <div class="split">
            <form class="box stack" on:submit=send>
                <div class="row wrap">
                    <span class="hint">"Start from:"</span>
                    <button type="button" class="btn ghost small" on:click=move |_| preset("Our usual chat is down. Use SpareLine for now.", "Our usual chat service is unavailable. Please use SpareLine for work conversations until we tell you it's back.")>"Chat is down"</button>
                    <button type="button" class="btn ghost small" on:click=move |_| preset("Email is down", "Email is not working at the moment. Use SpareLine to reach colleagues.")>"Email is down"</button>
                    <button type="button" class="btn ghost small" on:click=move |_| { preset("Our usual tools are back", "Chat and email are working again. You can switch back; SpareLine stays available."); sev.set(Severity::Info); }>"Services are back"</button>
                </div>
                <div class="field">
                    <label for="a-title">"Headline"</label>
                    <input id="a-title" class="input" maxlength="120" prop:value=move || title.get() on:input=move |e| title.set(event_target_value(&e))/>
                </div>
                <div class="field">
                    <label for="a-body">"Details (optional)"</label>
                    <textarea id="a-body" class="textarea" rows="3" maxlength="2000" prop:value=move || body.get() on:input=move |e| body.set(event_target_value(&e))></textarea>
                </div>
                <fieldset style="border:0;padding:0;margin:0">
                    <legend class="label" style="margin-bottom:var(--s2)">"Importance"</legend>
                    <div class="seg" role="radiogroup" aria-label="Importance">
                        {sev_opt(Severity::Info, "Information")}
                        {sev_opt(Severity::Warning, "Important")}
                        {sev_opt(Severity::Critical, "Urgent")}
                    </div>
                    <p class="hint" style="margin-top:var(--s1)">"People can dismiss information and important banners. Urgent ones stay until you end them."</p>
                </fieldset>
                <div class="field">
                    <label for="a-ends">"Ends"</label>
                    <select id="a-ends" class="select" style="width:auto" on:change=move |e| ends.set(event_target_value(&e).parse().unwrap_or(0))>
                        <option value="0">"When I end it"</option>
                        <option value="3600">"After 1 hour"</option>
                        <option value="14400">"After 4 hours"</option>
                        <option value="86400">"After 1 day"</option>
                    </select>
                </div>
                <div>
                    <span class="label">"Preview"</span>
                    <div class="banner" data-level=level style="margin-top:var(--s2);border:1px solid var(--rule);border-radius:var(--r-md)">
                        <Icon name="megaphone"/>
                        <div class="grow">
                            <strong>{move || if title.get().is_empty() { "Your headline".to_string() } else { title.get() }}</strong>
                            <Show when=move || !body.get().is_empty()><p>{move || body.get()}</p></Show>
                        </div>
                    </div>
                </div>
                <div><button class="btn" type="submit"><Icon name="megaphone"/>"Send announcement"</button></div>
            </form>
            <aside class="drawer">
                <h2>"Sent"</h2>
                {move || match list.get() {
                    None => view! { <sl_ui::Spinner/> }.into_any(),
                    Some(l) if l.is_empty() => view! { <p class="muted" style="margin-top:var(--s3)">"Nothing sent yet."</p> }.into_any(),
                    Some(l) => view! {
                        <ul class="mini-list" style="margin-top:var(--s3)">
                            {l.into_iter().map(|b| {
                                let id = b.broadcast.id;
                                view! {
                                    <li style="align-items:flex-start">
                                        <span class="grow">
                                            {if b.active { view! { <span class="tag ok">"Showing"</span> }.into_any() } else { view! { <span class="tag">"Ended"</span> }.into_any() }}
                                            " " <strong>{b.broadcast.title.clone()}</strong>
                                            <span class="hint" style="display:block">{format!("Sent {}{}", format::full(b.broadcast.created_at), b.broadcast.expires_at.filter(|_| b.active).map(|e| format!(", ends {}", format::until(e))).unwrap_or_default())}</span>
                                        </span>
                                        {b.active.then(|| view! {
                                            <button class="btn quiet small" on:click=move |_| {
                                                #[derive(Serialize)] struct A { id: uuid::Uuid }
                                                act("end_broadcast", A { id }, "Announcement ended.", move || { load(); poll_overview(); });
                                            }>"End"</button>
                                        })}
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }.into_any(),
                }}
            </aside>
        </div>
    }
}
