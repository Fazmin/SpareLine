//! Settings: profile, appearance, notifications, privacy, connection, about.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use sl_proto::client_ipc::*;
use sl_ui::{AppearanceControls, Icon, Modal, Switch, format, ipc, toasts};

use crate::state::{conn_label, save_notify, store};

#[derive(Deserialize, Clone, Default)]
struct AppInfo {
    version: String,
    platform: String,
    data_dir: String,
}

#[component]
pub fn Settings() -> impl IntoView {
    let s = store();
    // Jump to a section if asked (e.g. "Details" on the connection banner).
    Effect::new(move |_| {
        if let Some(id) = s.settings_section.get() {
            request_animation_frame(move || {
                if let Some(el) = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.get_element_by_id(id))
                {
                    el.scroll_into_view();
                }
            });
            s.settings_section.set(None);
        }
    });
    view! {
        <main class="page" id="main" tabindex="-1">
            <crate::shell::Banners/>
            <div class="page-inner">
                <h1>"Settings"</h1>
                <nav aria-label="Settings sections" class="row wrap" style="gap:var(--s2);margin-top:var(--s3)">
                    <a class="tag" href="#profile">"Profile"</a>
                    <a class="tag" href="#appearance">"Appearance"</a>
                    <a class="tag" href="#notifications">"Notifications"</a>
                    <a class="tag" href="#privacy">"Privacy"</a>
                    <a class="tag" href="#connection">"Connection"</a>
                    <a class="tag" href="#about">"About"</a>
                </nav>
                <Profile/>
                <section class="section" id="appearance" aria-labelledby="h-appearance">
                    <h2 id="h-appearance">"Appearance and accessibility"</h2>
                    <div class="section-body"><AppearanceControls/></div>
                </section>
                <Notifications/>
                <Privacy/>
                <Connection/>
                <About/>
            </div>
        </main>
    }
}

#[component]
fn Profile() -> impl IntoView {
    let s = store();
    let me = move || s.app.get().me;
    let name = RwSignal::new(me().map(|m| m.display_name).unwrap_or_default());
    let current = RwSignal::new(String::new());
    let new = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let save_name = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        #[derive(Serialize)]
        struct A {
            name: String,
        }
        let n = name.get_untracked();
        spawn_local(async move {
            match ipc::invoke::<()>("set_display_name", A { name: n }).await {
                Ok(()) => toasts().info("Name updated."),
                Err(e) => toasts().error(e.message),
            }
        });
    };
    let change_pw = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        if new.get_untracked().chars().count() < 10 {
            toasts().error("Choose a password of at least 10 characters.");
            return;
        }
        if new.get_untracked() != confirm.get_untracked() {
            toasts().error("The new passwords don't match.");
            return;
        }
        #[derive(Serialize)]
        struct A {
            current: String,
            new: String,
        }
        let a = A {
            current: current.get_untracked(),
            new: new.get_untracked(),
        };
        spawn_local(async move {
            match ipc::invoke::<()>("change_password", a).await {
                Ok(()) => {
                    toasts().info("Password changed. Your other devices were signed out.");
                    current.set(String::new());
                    new.set(String::new());
                    confirm.set(String::new());
                }
                Err(e) => toasts().error(e.message),
            }
        });
    };
    view! {
        <section class="section" id="profile" aria-labelledby="h-profile">
            <h2 id="h-profile">"Profile"</h2>
            <div class="section-body stack" style="gap:var(--s5)">
                <form class="stack" on:submit=save_name>
                    <div class="field">
                        <label for="p-name">"Display name"</label>
                        <div class="row">
                            <input id="p-name" class="input" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/>
                            <button class="btn" type="submit">"Save"</button>
                        </div>
                        <span class="hint">{move || me().map(|m| format!("Username @{} (can't be changed)", m.username)).unwrap_or_default()}</span>
                    </div>
                </form>
                <form class="stack" on:submit=change_pw>
                    <h3>"Change password"</h3>
                    <div class="field"><label for="p-cur">"Current password"</label><input id="p-cur" class="input" type="password" autocomplete="current-password" prop:value=move || current.get() on:input=move |e| current.set(event_target_value(&e))/></div>
                    <div class="field"><label for="p-new">"New password"</label><input id="p-new" class="input" type="password" autocomplete="new-password" prop:value=move || new.get() on:input=move |e| new.set(event_target_value(&e))/></div>
                    <div class="field"><label for="p-new2">"Confirm new password"</label><input id="p-new2" class="input" type="password" autocomplete="new-password" prop:value=move || confirm.get() on:input=move |e| confirm.set(event_target_value(&e))/></div>
                    <div><button class="btn quiet" type="submit">"Change password"</button></div>
                </form>
            </div>
        </section>
    }
}

#[component]
fn Notifications() -> impl IntoView {
    let s = store();
    let autostart = RwSignal::new(false);
    spawn_local(async move {
        if let Ok(v) = ipc::invoke0::<bool>("autostart_enabled").await {
            autostart.set(v);
        }
    });
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
        <section class="section" id="notifications" aria-labelledby="h-notif">
            <h2 id="h-notif">"Notifications"</h2>
            <div class="section-body stack" style="gap:var(--s4)">
                <Switch
                    label="Show notifications"
                    hint="When a message arrives and you're not looking at that chat."
                    checked=Signal::derive(move || s.notify.get().enabled)
                    on_change=Callback::new(move |v| { s.notify.update(|p| p.enabled = v); save_notify(&s.notify.get_untracked()); })
                />
                <Switch
                    label="Show message text in notifications"
                    hint="Turn off if others can see your screen."
                    checked=Signal::derive(move || s.notify.get().preview)
                    on_change=Callback::new(move |v| { s.notify.update(|p| p.preview = v); save_notify(&s.notify.get_untracked()); })
                />
                <Switch
                    label="Start SpareLine when I sign in to my computer"
                    hint="So it's ready the moment your usual chat goes down. It waits quietly in the system tray."
                    checked=Signal::derive(move || autostart.get())
                    on_change=set_auto
                />
            </div>
        </section>
    }
}

#[component]
fn Privacy() -> impl IntoView {
    let receipts = RwSignal::new(true);
    spawn_local(async move {
        if let Ok(v) = ipc::invoke0::<bool>("read_receipts").await {
            receipts.set(v);
        }
    });
    let set = Callback::new(move |on: bool| {
        receipts.set(on);
        #[derive(Serialize)]
        struct A {
            on: bool,
        }
        spawn_local(async move {
            let _ = ipc::invoke::<()>("set_read_receipts", A { on }).await;
        });
    });
    view! {
        <section class="section" id="privacy" aria-labelledby="h-privacy">
            <h2 id="h-privacy">"Privacy"</h2>
            <div class="section-body stack" style="gap:var(--s4)">
                <div class="notice ok">
                    <Icon name="shield"/>
                    <div>
                        <p><strong>"Your messages are end-to-end encrypted."</strong></p>
                        <p class="small">"They're encrypted on this computer and can only be read on the devices of the people in the chat. The SpareLine server and your admin can't read them, and the copy stored on this computer is encrypted too."</p>
                    </div>
                </div>
                <details>
                    <summary>"What the server can see"</summary>
                    <p class="small" style="margin-top:var(--s2)">"To deliver messages, the server knows your account, your groups, who is in each chat, when messages are sent and how big they are, whether you're online, when you're typing, and how far you've read. It never sees message text, reactions, edits or chat names."</p>
                </details>
                <Switch
                    label="Send read receipts"
                    hint="Let others see when you've read their messages."
                    checked=Signal::derive(move || receipts.get())
                    on_change=set
                />
            </div>
        </section>
    }
}

fn report(d: &Diagnostics) -> String {
    let mut out = String::new();
    out.push_str("SpareLine connection report\n");
    out.push_str(&format!("App {} on {}\n", d.app_version, d.platform));
    if let Some(s) = &d.server {
        out.push_str(&format!(
            "Server: {} at {}\nFingerprint: {}\n",
            s.name, s.last_address, s.fingerprint
        ));
    }
    out.push_str(&format!(
        "State: {}\n\nLast attempt:\n",
        conn_label(&d.conn, "server")
    ));
    for st in &d.last_attempt {
        out.push_str(&format!(
            "  [{}] {} ({} ms) {}\n",
            if st.ok { "ok" } else { "FAILED" },
            st.name,
            st.millis,
            st.detail
        ));
    }
    out.push_str("\nRecent problems:\n");
    for (k, n) in &d.recent_failures {
        out.push_str(&format!("  {k}: {n}\n"));
    }
    out.push_str("\nServers found on the network:\n");
    for s in &d.discovered {
        out.push_str(&format!("  {} {}\n", s.name, s.addresses.join(", ")));
    }
    out.push_str("\nLog:\n");
    for l in &d.log {
        out.push_str(l);
        out.push('\n');
    }
    out
}

#[component]
fn Connection() -> impl IntoView {
    let s = store();
    let diag = RwSignal::new(None::<Diagnostics>);
    let load = move || {
        spawn_local(async move {
            if let Ok(d) = ipc::invoke0::<Diagnostics>("diagnostics").await {
                diag.set(Some(d));
            }
        });
    };
    load();
    if let Ok(h) = set_interval_with_handle(load, std::time::Duration::from_secs(3)) {
        on_cleanup(move || h.clear());
    }
    let copy = move |_| {
        let Some(d) = diag.get_untracked() else {
            return;
        };
        let text = report(&d);
        spawn_local(async move {
            if ipc::copy_text(&text).await {
                toasts().info("Report copied. Paste it into a message or email to your admin.");
            } else {
                toasts().error("Couldn't copy to the clipboard.");
            }
        });
    };
    let retry = move |_| {
        spawn_local(async move {
            let _ = ipc::invoke0::<()>("reconnect_now").await;
        });
    };
    view! {
        <section class="section" id="connection" aria-labelledby="h-conn">
            <h2 id="h-conn">"Connection"</h2>
            <div class="section-body stack" style="gap:var(--s4)">
                <p style="font-weight:700" aria-live="polite">
                    {move || conn_label(&s.app.get().conn, &s.app.get().server.map(|x| x.name).unwrap_or_else(|| "the server".into()))}
                </p>
                <dl class="kv">
                    <dt>"Server"</dt><dd>{move || s.app.get().server.map(|x| x.name).unwrap_or_default()}</dd>
                    <dt>"Address"</dt><dd class="mono">{move || s.app.get().server.map(|x| x.last_address).unwrap_or_default()}</dd>
                    <dt>"Fingerprint"</dt><dd class="fingerprint">{move || s.app.get().server.map(|x| x.fingerprint).unwrap_or_default()}</dd>
                </dl>
                <p class="hint">"Your admin can confirm the fingerprint in the SpareLine Server control panel. If it ever changes unexpectedly, SpareLine stops connecting."</p>
                {move || diag.get().map(|d| view! {
                    <div>
                        <h3 style="font-size:var(--t-md)">"Last connection attempt"</h3>
                        <ul class="steps">
                            {d.last_attempt.iter().map(|st| view! {
                                <li>
                                    <span class=if st.ok { "ok" } else { "bad" }><Icon name=if st.ok { "check" } else { "alert" }/></span>
                                    <span class="grow">{st.name.clone()}<span class="hint" style="display:block">{st.detail.clone()}</span></span>
                                    <span class="hint">{format!("{} ms", st.millis)}</span>
                                </li>
                            }).collect_view()}
                        </ul>
                        {(!d.recent_failures.is_empty()).then(|| view! {
                            <p class="small" style="margin-top:var(--s3)">
                                "Recent problems: " {d.recent_failures.iter().map(|(k, n)| format!("{} ({n})", k.replace('_', " "))).collect::<Vec<_>>().join(", ")}
                            </p>
                        })}
                        <details style="margin-top:var(--s3)">
                            <summary>"Technical log"</summary>
                            <pre class="log">{d.log.join("\n")}</pre>
                        </details>
                    </div>
                })}
                <div class="row wrap">
                    <button class="btn quiet" on:click=retry><Icon name="refresh"/>"Reconnect now"</button>
                    <button class="btn quiet" on:click=copy><Icon name="copy"/>"Copy report for your admin"</button>
                </div>
            </div>
        </section>
    }
}

#[component]
fn About() -> impl IntoView {
    let info = RwSignal::new(AppInfo::default());
    spawn_local(async move {
        if let Ok(i) = ipc::invoke0::<AppInfo>("app_info").await {
            info.set(i);
        }
    });
    let forget_open = RwSignal::new(false);
    let sign_out = move |forget: bool| {
        #[derive(Serialize)]
        struct A {
            forget: bool,
        }
        spawn_local(async move {
            match ipc::invoke::<AppState>("logout", A { forget }).await {
                Ok(st) => store().app.set(st),
                Err(e) => toasts().error(e.message),
            }
        });
    };
    view! {
        <section class="section" id="about" aria-labelledby="h-about">
            <h2 id="h-about">"About and sign out"</h2>
            <div class="section-body stack" style="gap:var(--s4)">
                <dl class="kv">
                    <dt>"Version"</dt><dd>{move || info.get().version}</dd>
                    <dt>"System"</dt><dd>{move || info.get().platform}</dd>
                    <dt>"Data folder"</dt><dd class="mono small">{move || info.get().data_dir}</dd>
                    <dt>"Today"</dt><dd>{format::day_label(format::now())}</dd>
                </dl>
                <div class="row wrap">
                    <button class="btn quiet" on:click=move |_| sign_out(false)><Icon name="logout"/>"Sign out"</button>
                    <button class="btn ghost" on:click=move |_| forget_open.set(true)>"Remove this server from this computer"</button>
                </div>
            </div>
            <Modal open=forget_open title="Remove this server?">
                <div class="modal-body"><p>"This signs you out and deletes the saved server and your chat history from this computer. Your account on the server stays."</p></div>
                <div class="modal-foot">
                    <button class="btn quiet" on:click=move |_| forget_open.set(false)>"Cancel"</button>
                    <button class="btn danger" on:click=move |_| { forget_open.set(false); sign_out(true); }>"Remove"</button>
                </div>
            </Modal>
        </section>
    }
}
