//! Settings → Standby copy: keep an encrypted copy of this server somewhere else, so
//! another computer can take over if this one fails.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::panel::{StandbyCopyRow, StandbySetupInput, StandbyStatus};
use sl_ui::{Icon, Spinner, format, toasts};

use crate::ctx::{call, call0};

const EVERY: [(u32, &str); 6] = [
    (5, "5 minutes"),
    (15, "15 minutes"),
    (30, "30 minutes"),
    (60, "hour"),
    (240, "4 hours"),
    (1440, "day"),
];

#[component]
pub fn StandbySection() -> impl IntoView {
    let st = RwSignal::new(None::<StandbyStatus>);
    let folder = RwSignal::new(String::new());
    let every = RwSignal::new(15u32);
    let keep = RwSignal::new(4u32);
    let pw = RwSignal::new(String::new());
    let pw2 = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    let load = move |s: StandbyStatus| {
        if s.enabled {
            folder.set(s.folder.clone());
        }
        if s.interval_minutes > 0 {
            every.set(s.interval_minutes);
        }
        if s.keep > 0 {
            keep.set(s.keep);
        }
        st.set(Some(s));
    };
    let reload = move || {
        spawn_local(async move {
            if let Ok(s) = call0::<StandbyStatus>("standby_status").await {
                load(s);
            }
        });
    };
    reload();

    let enabled = move || st.get().is_some_and(|s| s.enabled);
    let has_pw = move || st.get().is_some_and(|s| s.has_password);

    let save = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        error.set(None);
        let p = pw.get_untracked();
        if folder.get_untracked().trim().is_empty() {
            error.set(Some("Choose a folder for the copies.".into()));
            return;
        }
        if p.is_empty() && !st.get_untracked().is_some_and(|s| s.has_password) {
            error.set(Some(
                "Choose a standby password. You'll need it to restore a copy.".into(),
            ));
            return;
        }
        if !p.is_empty() && p.chars().count() < 10 {
            error.set(Some(
                "Use at least 10 characters for the standby password.".into(),
            ));
            return;
        }
        if p != pw2.get_untracked() {
            error.set(Some("The two passwords don't match.".into()));
            return;
        }
        busy.set(true);
        #[derive(Serialize)]
        struct A {
            input: StandbySetupInput,
        }
        let input = StandbySetupInput {
            folder: folder.get_untracked().trim().to_string(),
            interval_minutes: every.get_untracked(),
            keep: keep.get_untracked(),
            password: p,
        };
        spawn_local(async move {
            match call::<StandbyStatus>("standby_setup", A { input }).await {
                Ok(s) => {
                    pw.set(String::new());
                    pw2.set(String::new());
                    if s.last_error.is_none() {
                        toasts().info("Standby copies are on. The first copy is saved.");
                    }
                    load(s);
                }
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    let copy_now = move |_| {
        busy.set(true);
        spawn_local(async move {
            match call0::<StandbyCopyRow>("standby_copy_now").await {
                Ok(r) => toasts().info(format!("Standby copy saved ({}).", format::bytes(r.size))),
                Err(e) => toasts().error(e.message),
            }
            if let Ok(s) = call0::<StandbyStatus>("standby_status").await {
                load(s);
            }
            busy.set(false);
        });
    };
    let turn_off = move |_| {
        spawn_local(async move {
            match call0::<()>("standby_turn_off").await {
                Ok(()) => {
                    toasts().info("Standby copies are off. Copies already made stay in the folder.")
                }
                Err(e) => toasts().error(e.message),
            }
            if let Ok(s) = call0::<StandbyStatus>("standby_status").await {
                load(s);
            }
        });
    };
    let choose = move |_| {
        spawn_local(async move {
            if let Ok(Some(f)) = call0::<Option<String>>("pick_folder").await {
                folder.set(f);
            }
        });
    };

    let status = move || {
        let s = st.get()?;
        if !s.enabled {
            return None;
        }
        Some(if let Some(e) = s.last_error.clone() {
            view! {
                <div class="notice fail" role="status"><Icon name="alert"/><span>{format!("The last copy failed. {e}")}</span></div>
            }
            .into_any()
        } else if s.same_disk {
            view! {
                <div class="notice warn" role="status"><Icon name="alert"/><span>"This folder is on the same disk as the server, so the copies wouldn't survive a disk failure. Choose a USB drive or a shared folder on another computer."</span></div>
            }
            .into_any()
        } else if let Some(c) = s.last_copy.clone() {
            let every = EVERY
                .iter()
                .find(|(m, _)| *m == s.interval_minutes)
                .map_or_else(
                    || format!("{} minutes", s.interval_minutes),
                    |(_, l)| (*l).to_string(),
                );
            view! {
                <div class="notice ok" role="status"><Icon name="check"/><span>{format!(
                    "On. Last copy {} ({}). A new copy is made every {every}, and soon after you change people, groups or invites.",
                    format::ago(c.created_at),
                    format::bytes(c.size)
                )}</span></div>
            }
            .into_any()
        } else {
            view! {
                <div class="notice" role="status"><Icon name="clock"/><span>"On. The first copy will be made shortly."</span></div>
            }
            .into_any()
        })
    };

    view! {
        <section class="section" aria-labelledby="h-standby">
            <div class="section-title">
                <h2 id="h-standby">"Standby copy"</h2>
                <Show when=enabled>
                    <button class="btn small" disabled=move || busy.get() on:click=copy_now><Icon name="download"/>"Make a copy now"</button>
                </Show>
            </div>
            <form class="box stack" on:submit=save novalidate>
                <p class="small muted">"Keeps an up-to-date, encrypted copy of this server somewhere else, such as a USB drive or a shared folder on another computer. If this computer stops working, install SpareLine Server on another one and choose "<b>"Restore a standby copy"</b>". Staff apps reconnect by themselves."</p>
                {status}
                <div class="field">
                    <label for="sb-folder">"Folder for the copies"</label>
                    <div class="row">
                        <input id="sb-folder" class="input grow" placeholder="For example E:\\SpareLine or \\\\fileserver\\spareline" prop:value=move || folder.get() on:input=move |e| folder.set(event_target_value(&e))/>
                        <button type="button" class="btn quiet" on:click=choose>"Choose…"</button>
                    </div>
                    <span class="hint">"Pick somewhere that would survive this computer failing."</span>
                </div>
                <div class="row wrap" style="align-items:flex-start;gap:var(--s5)">
                    <div class="field">
                        <label for="sb-every">"Make a copy every"</label>
                        <select id="sb-every" class="select" style="width:auto" on:change=move |e| every.set(event_target_value(&e).parse().unwrap_or(15))>
                            {EVERY.iter().map(|(m, label)| {
                                let m = *m;
                                view! { <option value=m.to_string() selected=move || every.get() == m>{*label}</option> }
                            }).collect_view()}
                        </select>
                    </div>
                    <div class="field">
                        <label for="sb-keep">"Copies to keep"</label>
                        <input id="sb-keep" class="input" type="number" min="1" max="50" style="width:7rem" prop:value=move || keep.get().to_string() on:input=move |e| keep.set(event_target_value(&e).parse().unwrap_or(4))/>
                    </div>
                </div>
                <div class="row wrap" style="align-items:flex-start;gap:var(--s5)">
                    <div class="field">
                        <label for="sb-pw">"Standby password"</label>
                        <input id="sb-pw" class="input" type="password" autocomplete="new-password"
                            placeholder=move || if has_pw() { "Leave blank to keep it" } else { "" }
                            prop:value=move || pw.get() on:input=move |e| pw.set(event_target_value(&e))/>
                    </div>
                    <div class="field">
                        <label for="sb-pw2">"Confirm standby password"</label>
                        <input id="sb-pw2" class="input" type="password" autocomplete="new-password" prop:value=move || pw2.get() on:input=move |e| pw2.set(event_target_value(&e))/>
                    </div>
                </div>
                <span class="hint">"Needed to restore a copy. Store it with the recovery key, somewhere other than this computer."</span>
                <Show when=move || error.get().is_some()>
                    <div class="notice fail" role="alert"><Icon name="alert"/><span>{move || error.get().unwrap_or_default()}</span></div>
                </Show>
                <div class="row">
                    <button class="btn" type="submit" disabled=move || busy.get()>
                        {move || if busy.get() { view! { <Spinner/> "Copying…" }.into_any() } else if enabled() { "Save and make a copy".into_any() } else { "Turn on standby copies".into_any() }}
                    </button>
                    <Show when=enabled>
                        <button type="button" class="btn quiet" on:click=turn_off>"Turn off"</button>
                    </Show>
                </div>
                <Show when=move || st.get().is_some_and(|s| s.enabled && !s.copies.is_empty())>
                    <table class="table">
                        <caption class="sr-only">"Standby copies in the folder"</caption>
                        <thead><tr><th scope="col">"Copy"</th><th scope="col">"Made"</th><th scope="col">"Size"</th></tr></thead>
                        <tbody>
                            {move || st.get().map(|s| s.copies).unwrap_or_default().into_iter().map(|c| view! {
                                <tr><td class="mono small">{c.file_name}</td><td>{format::full(c.created_at)}</td><td>{format::bytes(c.size)}</td></tr>
                            }).collect_view()}
                        </tbody>
                    </table>
                </Show>
            </form>
        </section>
    }
}
