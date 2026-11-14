//! Settings: server, retention, backups, admin password, appearance, about.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use sl_proto::panel::*;
use sl_ui::{AppearanceControls, Icon, Switch, format, ipc, toasts};

use crate::ctx::{act, call, call0, ctx};

#[derive(Deserialize, Clone, Default)]
struct AppInfo {
    version: String,
    data_dir: String,
    platform: String,
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let c = ctx();
    let s = RwSignal::new(None::<ServerSettings>);
    let backups = RwSignal::new(Vec::<BackupRow>::new());
    let info = RwSignal::new(AppInfo::default());
    let autostart = RwSignal::new(false);
    let restart_needed = RwSignal::new(false);
    let cur = RwSignal::new(String::new());
    let new = RwSignal::new(String::new());
    spawn_local(async move {
        if let Ok(v) = call0::<ServerSettings>("settings").await {
            s.set(Some(v));
        }
        if let Ok(b) = call0::<Vec<BackupRow>>("backups").await {
            backups.set(b);
        }
        if let Ok(i) = ipc::invoke0::<AppInfo>("app_info").await {
            info.set(i);
        }
        if let Ok(a) = ipc::invoke0::<bool>("autostart_enabled").await {
            autostart.set(a);
        }
    });
    let field = move |label: &'static str,
                      hint: &'static str,
                      get: fn(&ServerSettings) -> u32,
                      set: fn(&mut ServerSettings, u32)| {
        let id = sl_ui::uid("set");
        let id2 = id.clone();
        view! {
            <div class="field">
                <label for=id>{label}</label>
                <input id=id2 class="input" type="number" min="1" style="width:9rem"
                    prop:value=move || s.get().map(|x| get(&x).to_string()).unwrap_or_default()
                    on:input=move |e| { let v: u32 = event_target_value(&e).parse().unwrap_or(1); s.update(|x| if let Some(x) = x { set(x, v) }); }/>
                <span class="hint">{hint}</span>
            </div>
        }
    };
    let save = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let Some(input) = s.get_untracked() else {
            return;
        };
        #[derive(Serialize)]
        struct A {
            input: ServerSettings,
        }
        spawn_local(async move {
            match call::<bool>("update_settings", A { input }).await {
                Ok(restart) => {
                    restart_needed.set(restart);
                    toasts().info(if restart {
                        "Saved. Restart the server to use the new port."
                    } else {
                        "Settings saved."
                    });
                }
                Err(e) => toasts().error(e.message),
            }
        });
    };
    let restart = move |_| {
        act(
            "restart_server",
            serde_json::json!({}),
            "Server restarted.",
            move || restart_needed.set(false),
        );
    };
    let backup_now = move |_| {
        spawn_local(async move {
            match call0::<BackupRow>("backup_now").await {
                Ok(b) => {
                    toasts().info(format!("Backup saved ({}).", format::bytes(b.size)));
                    if let Ok(list) = call0::<Vec<BackupRow>>("backups").await {
                        backups.set(list);
                    }
                }
                Err(e) => toasts().error(e.message),
            }
        });
    };
    let change_pw = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        #[derive(Serialize)]
        struct A {
            current: String,
            new: String,
        }
        act(
            "change_admin_password",
            A {
                current: cur.get_untracked(),
                new: new.get_untracked(),
            },
            "Admin password changed.",
            move || {
                cur.set(String::new());
                new.set(String::new());
            },
        );
    };
    let set_auto = Callback::new(move |on: bool| {
        autostart.set(on);
        #[derive(Serialize)]
        struct A {
            on: bool,
        }
        spawn_local(async move {
            if let Err(e) = ipc::invoke::<()>("set_autostart", A { on }).await {
                toasts().error(e.message);
            }
        });
    });
    view! {
        <div class="page-head">
            <div class="grow">
                <h1>"Settings"</h1>
                <p class="lede">"Server options, backups, the admin password and how this panel looks."</p>
            </div>
        </div>
        <section aria-labelledby="h-server">
            <div class="section-title"><h2 id="h-server">"Server"</h2></div>
            <form class="box stack" on:submit=save>
                <div class="field">
                    <label for="st-name">"Server name"</label>
                    <input id="st-name" class="input" prop:value=move || s.get().map(|x| x.server_name).unwrap_or_default() on:input=move |e| { let v = event_target_value(&e); s.update(|x| if let Some(x) = x { x.server_name = v }); }/>
                    <span class="hint">"Shown to staff when SpareLine finds this server."</span>
                </div>
                <div class="row wrap" style="align-items:flex-start;gap:var(--s5)">
                    {field("Port", "Staff devices connect on this port. Changing it needs a restart and a firewall update.", |x| x.port as u32, |x, v| x.port = v.min(65535) as u16)}
                    {field("Keep relayed messages (days)", "Encrypted copies wait this long for devices that are offline.", |x| x.envelope_retention_days, |x, v| x.envelope_retention_days = v)}
                    {field("Sign people out after (days unused)", "A device that doesn't connect for this long must sign in again.", |x| x.session_idle_days, |x, v| x.session_idle_days = v)}
                    {field("Keep logs (days)", "", |x| x.log_retention_days, |x, v| x.log_retention_days = v)}
                    {field("Backups to keep", "", |x| x.backup_keep, |x, v| x.backup_keep = v)}
                </div>
                <div class="row">
                    <button class="btn" type="submit">"Save settings"</button>
                    <Show when=move || restart_needed.get()>
                        <button type="button" class="btn quiet" on:click=restart><Icon name="refresh"/>"Restart server now"</button>
                    </Show>
                </div>
                <Switch label="Start SpareLine Server when this computer starts" hint="Recommended, so the backup chat is always available." checked=Signal::derive(move || autostart.get()) on_change=set_auto/>
            </form>
        </section>
        <section class="section" aria-labelledby="h-backups">
            <div class="section-title">
                <h2 id="h-backups">"Backups"</h2>
                <button class="btn quiet small" on:click=move |_| act("open_data_folder", serde_json::json!({}), "", || {})><Icon name="server"/>"Open data folder"</button>
                <button class="btn small" on:click=backup_now><Icon name="download"/>"Back up now"</button>
            </div>
            <div class="box flush">
                <p class="small muted" style="padding:var(--s4) var(--s5) 0">"A backup is made every day. It contains accounts, groups and encrypted message copies; message content can't be read from it."</p>
                <table class="table">
                    <thead><tr><th scope="col">"File"</th><th scope="col">"Made"</th><th scope="col">"Size"</th></tr></thead>
                    <tbody>
                        {move || {
                            let b = backups.get();
                            if b.is_empty() {
                                view! { <tr><td colspan="3" class="hint">"No backups yet."</td></tr> }.into_any()
                            } else {
                                b.into_iter().map(|x| view! { <tr><td class="mono small">{x.file_name}</td><td>{format::full(x.created_at)}</td><td>{format::bytes(x.size)}</td></tr> }).collect_view().into_any()
                            }
                        }}
                    </tbody>
                </table>
            </div>
        </section>
        <section class="section" aria-labelledby="h-admin">
            <div class="section-title"><h2 id="h-admin">"Admin password"</h2></div>
            <form class="box stack" on:submit=change_pw>
                <div class="row wrap" style="align-items:flex-end">
                    <div class="field"><label for="ap-cur">"Current password"</label><input id="ap-cur" class="input" type="password" autocomplete="current-password" prop:value=move || cur.get() on:input=move |e| cur.set(event_target_value(&e))/></div>
                    <div class="field"><label for="ap-new">"New password"</label><input id="ap-new" class="input" type="password" autocomplete="new-password" prop:value=move || new.get() on:input=move |e| new.set(event_target_value(&e))/></div>
                    <button class="btn quiet" type="submit">"Change password"</button>
                </div>
                <p class="hint">"The panel locks itself after 15 minutes without use, and when you close the window."</p>
            </form>
        </section>
        <section class="section" aria-labelledby="h-appear">
            <div class="section-title"><h2 id="h-appear">"Appearance and accessibility"</h2></div>
            <div class="box"><AppearanceControls/></div>
        </section>
        <section class="section" aria-labelledby="h-about">
            <div class="section-title"><h2 id="h-about">"About"</h2></div>
            <div class="box">
                <dl class="sys">
                    <div><dt>"Version"</dt><dd>{move || info.get().version}</dd></div>
                    <div><dt>"System"</dt><dd>{move || info.get().platform}</dd></div>
                    <div><dt>"Fingerprint"</dt><dd class="mono small">{move || c.overview.get().map(|o| o.o.fingerprint).unwrap_or_default()}</dd></div>
                    <div><dt>"Data folder"</dt><dd class="mono small" style="overflow-wrap:anywhere">{move || info.get().data_dir}</dd></div>
                </dl>
                <p class="privacy-note"><Icon name="shield"/><span>"SpareLine uses end-to-end encryption (MLS). This server stores account details and encrypted message copies only. It has no way to decrypt conversations, and this panel has no way to show them."</span></p>
            </div>
        </section>
    }
}
