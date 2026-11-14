//! First-run setup (or restoring a standby copy) and the lock screen.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::panel::{PanelState, RestoreResult, SetupInput, SetupResult, StandbyInfo};
use sl_ui::{Icon, Spinner, format, ipc, toasts};

use crate::ctx::{call0, ctx};

fn hero(title: &'static str, body: &'static str) -> impl IntoView {
    view! {
        <section class="gate-hero">
            <div class="gate-lines" aria-hidden="true"><i></i><i></i></div>
            <h1>{title}</h1>
            <p>{body}</p>
        </section>
    }
}

async fn refresh() {
    if let Ok(s) = call0::<PanelState>("panel_state").await {
        ctx().state.set(Some(s));
    }
}

#[component]
pub fn Setup() -> impl IntoView {
    let c = ctx();
    let name = RwSignal::new(
        c.state
            .get_untracked()
            .map(|s| s.suggested_name)
            .unwrap_or_default(),
    );
    let pw = RwSignal::new(String::new());
    let pw2 = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let key = RwSignal::new(None::<String>);
    let stored = RwSignal::new(false);
    let restoring = RwSignal::new(false);
    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        error.set(None);
        if pw.get_untracked().chars().count() < 10 {
            error.set(Some(
                "Use at least 10 characters for the admin password.".into(),
            ));
            return;
        }
        if pw.get_untracked() != pw2.get_untracked() {
            error.set(Some("The two passwords don't match.".into()));
            return;
        }
        busy.set(true);
        #[derive(Serialize)]
        struct A {
            input: SetupInput,
        }
        let input = SetupInput {
            server_name: name.get_untracked(),
            admin_password: pw.get_untracked(),
        };
        spawn_local(async move {
            match ipc::invoke::<SetupResult>("setup", A { input }).await {
                Ok(r) => key.set(Some(r.recovery_key)),
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    let copy = move |_| {
        let k = key.get_untracked().unwrap_or_default();
        spawn_local(async move {
            if ipc::copy_text(&k).await {
                toasts().info("Recovery key copied.");
            }
        });
    };
    let save = move |_| {
        let k = key.get_untracked().unwrap_or_default();
        #[derive(Serialize)]
        struct A {
            filename: String,
            content: String,
        }
        let content = format!(
            "SpareLine Server recovery key\n\n{k}\n\nUse it to reset the control panel password if it's forgotten.\nKeep it somewhere safe and offline.\n"
        );
        spawn_local(async move {
            let _ = ipc::invoke::<Option<String>>(
                "save_text",
                A {
                    filename: "spareline-recovery-key.txt".into(),
                    content,
                },
            )
            .await;
        });
    };
    view! {
        <div class="gate">
            {move || if restoring.get() {
                hero(
                    "Take over from a server that stopped working.",
                    "Restore the old server's standby copy on this computer. It keeps the same identity, accounts and groups, so staff apps reconnect without new invites.",
                )
            } else {
                hero(
                    "Set up your backup chat server.",
                    "This computer will run SpareLine for your organisation. Staff connect to it over the office network, and it keeps working when cloud services are down. Messages are end-to-end encrypted: neither this server nor you as admin can read them.",
                )
            }}
            <section class="gate-form" aria-label="Setup">
                <Show when=move || !restoring.get() fallback=move || view! { <RestoreForm back=Callback::new(move |()| restoring.set(false))/> }>
                <Show
                    when=move || key.get().is_none()
                    fallback=move || view! {
                        <div class="stack">
                            <h2>"Save your recovery key"</h2>
                            <p class="muted">"If you forget the admin password, this key is the only way back into the control panel. It's shown once."</p>
                            <div class="code-block" aria-label="Recovery key">{move || key.get().unwrap_or_default()}</div>
                            <div class="row">
                                <button class="btn quiet" on:click=copy><Icon name="copy"/>"Copy"</button>
                                <button class="btn quiet" on:click=save><Icon name="download"/>"Save to a file"</button>
                            </div>
                            <label class="row" style="cursor:pointer">
                                <input type="checkbox" style="width:1.1rem;height:1.1rem;accent-color:var(--signal)" on:change=move |e| stored.set(event_target_checked(&e))/>
                                "I've stored the recovery key somewhere safe"
                            </label>
                            <button class="btn wide" disabled=move || !stored.get() on:click=move |_| spawn_local(refresh())>"Open the control panel"</button>
                        </div>
                    }
                >
                    <form class="stack" on:submit=submit novalidate>
                        <h2>"Welcome"</h2>
                        <p class="muted">"Two things and you're done. The server's security certificate and database are created automatically."</p>
                        <div class="field">
                            <label for="s-name">"Server name"</label>
                            <input id="s-name" class="input" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/>
                            <span class="hint">"Staff see this name when SpareLine finds the server, for example \"Head office\"."</span>
                        </div>
                        <div class="field">
                            <label for="s-pw">"Admin password"</label>
                            <input id="s-pw" class="input" type="password" autocomplete="new-password" prop:value=move || pw.get() on:input=move |e| pw.set(event_target_value(&e))/>
                            <span class="hint">"Protects this control panel. At least 10 characters."</span>
                        </div>
                        <div class="field">
                            <label for="s-pw2">"Confirm admin password"</label>
                            <input id="s-pw2" class="input" type="password" autocomplete="new-password" prop:value=move || pw2.get() on:input=move |e| pw2.set(event_target_value(&e))/>
                        </div>
                        <Show when=move || error.get().is_some()>
                            <div class="notice fail" role="alert"><Icon name="alert"/><span>{move || error.get().unwrap_or_default()}</span></div>
                        </Show>
                        <button type="submit" class="btn wide" disabled=move || busy.get()>
                            {move || if busy.get() { view! { <Spinner/> "Setting up…" }.into_any() } else { "Set up and start the server".into_any() }}
                        </button>
                        <div class="gate-alt">
                            <span class="small muted">"Replacing a server that stopped working?"</span>
                            <button type="button" class="btn ghost small" on:click=move |_| restoring.set(true)>"Restore a standby copy"</button>
                        </div>
                    </form>
                </Show>
                </Show>
            </section>
        </div>
    }
}

/// Take over from a server that stopped working, using its standby copy.
#[component]
fn RestoreForm(back: Callback<()>) -> impl IntoView {
    let path = RwSignal::new(String::new());
    let pw = RwSignal::new(String::new());
    let info = RwSignal::new(None::<StandbyInfo>);
    let done = RwSignal::new(None::<RestoreResult>);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    #[derive(Serialize)]
    struct A {
        path: String,
        password: String,
    }
    let args = move || A {
        path: path.get_untracked().trim().to_string(),
        password: pw.get_untracked(),
    };
    let choose = move |_| {
        spawn_local(async move {
            if let Ok(Some(f)) = ipc::invoke0::<Option<String>>("pick_standby_copy").await {
                path.set(f);
            }
        });
    };
    let check = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        error.set(None);
        if path.get_untracked().trim().is_empty() {
            error.set(Some("Choose the standby copy to restore.".into()));
            return;
        }
        if pw.get_untracked().is_empty() {
            error.set(Some("Enter the standby password.".into()));
            return;
        }
        busy.set(true);
        let a = args();
        spawn_local(async move {
            match ipc::invoke::<StandbyInfo>("standby_inspect", a).await {
                Ok(i) => info.set(Some(i)),
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    let restore = move |_| {
        error.set(None);
        busy.set(true);
        let a = args();
        spawn_local(async move {
            match ipc::invoke::<RestoreResult>("standby_restore", a).await {
                Ok(r) => done.set(Some(r)),
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    let error_box = move || {
        error.get().map(|m| {
            view! { <div class="notice fail" role="alert"><Icon name="alert"/><span>{m}</span></div> }
        })
    };
    move || {
        if let Some(r) = done.get() {
            let taken = format::full(r.info.taken_at);
            let old = r.info.addresses.join(", ");
            let new = r.addresses.join(", ");
            let reach = if old.is_empty() || old == new {
                " when this computer is on the same network.".to_string()
            } else {
                format!(
                    " when this computer is on the same network. Staff who type the server's address need the old one, {old}: give this computer that address, or tell them the new one, {new}."
                )
            };
            view! {
                <div class="stack">
                    <h2>"This computer has taken over"</h2>
                    <p class="muted">{format!("{} is running here, from the copy made {taken}.", r.info.server_name)}</p>
                    <ol class="next-steps">
                        <li><b>"Keep the old server switched off."</b>" If it came back, two servers with the same identity would confuse staff apps."</li>
                        <li><b>"Staff apps reconnect by themselves"</b>{reach}</li>
                        <li><b>"Check what changed after the copy."</b>{format!(" Anything done after {taken} isn't in it. In People, check anyone you added, removed or signed out since then.")}</li>
                        <li><b>"Use the old server's admin password"</b>" to unlock this panel."</li>
                    </ol>
                    <button class="btn wide" on:click=move |_| spawn_local(refresh())>"Open the control panel"</button>
                </div>
            }
            .into_any()
        } else if let Some(i) = info.get() {
            let addresses = if i.addresses.is_empty() {
                "Not recorded".to_string()
            } else {
                i.addresses.join(", ")
            };
            view! {
                <div class="stack">
                    <h2>"Restore this copy?"</h2>
                    <table class="table kv">
                        <caption class="sr-only">"About this copy"</caption>
                        <tbody>
                            <tr><th scope="row">"Server"</th><td>{i.server_name.clone()}</td></tr>
                            <tr><th scope="row">"Copy made"</th><td>{format!("{} ({})", format::full(i.taken_at), format::ago(i.taken_at))}</td></tr>
                            <tr><th scope="row">"People"</th><td>{i.people}</td></tr>
                            <tr><th scope="row">"Old address"</th><td>{addresses}</td></tr>
                            <tr><th scope="row">"Fingerprint"</th><td class="mono small" style="white-space:normal;overflow-wrap:anywhere">{i.fingerprint.clone()}</td></tr>
                        </tbody>
                    </table>
                    <p class="muted small">"This computer takes over the old server's identity, settings and accounts, so staff apps trust it. Anything done after the copy was made isn't included."</p>
                    {error_box}
                    <button class="btn wide" disabled=move || busy.get() on:click=restore>
                        {move || if busy.get() { view! { <Spinner/> "Restoring…" }.into_any() } else { "Restore and start the server".into_any() }}
                    </button>
                    <button type="button" class="btn ghost" on:click=move |_| { info.set(None); error.set(None); }>"Choose a different copy"</button>
                </div>
            }
            .into_any()
        } else {
            view! {
                <form class="stack" on:submit=check novalidate>
                    <h2>"Restore a standby copy"</h2>
                    <p class="muted">"Use this when this computer is replacing a SpareLine Server that stopped working. It takes over with the same identity, so staff apps reconnect by themselves."</p>
                    <div class="field">
                        <label for="r-file">"Standby copy"</label>
                        <div class="row">
                            <input id="r-file" class="input grow" placeholder="spareline-standby-….slstandby" prop:value=move || path.get() on:input=move |e| path.set(event_target_value(&e))/>
                            <button type="button" class="btn quiet" on:click=choose>"Choose…"</button>
                        </div>
                        <span class="hint">"From the old server's standby folder. Choose the newest copy."</span>
                    </div>
                    <div class="field">
                        <label for="r-pw">"Standby password"</label>
                        <input id="r-pw" class="input" type="password" autocomplete="current-password" prop:value=move || pw.get() on:input=move |e| pw.set(event_target_value(&e))/>
                    </div>
                    {error_box}
                    <button type="submit" class="btn wide" disabled=move || busy.get()>
                        {move || if busy.get() { view! { <Spinner/> "Checking…" }.into_any() } else { "Check the copy".into_any() }}
                    </button>
                    <button type="button" class="btn ghost" on:click=move |_| back.run(())>"Back to setup"</button>
                </form>
            }
            .into_any()
        }
    }
}

#[component]
pub fn Unlock() -> impl IntoView {
    let c = ctx();
    let pw = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let resetting = RwSignal::new(false);
    let recovery = RwSignal::new(String::new());
    let new_pw = RwSignal::new(String::new());
    let new_key = RwSignal::new(None::<String>);
    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        error.set(None);
        busy.set(true);
        #[derive(Serialize)]
        struct A {
            password: String,
        }
        let password = pw.get_untracked();
        spawn_local(async move {
            match ipc::invoke::<()>("unlock", A { password }).await {
                Ok(()) => refresh().await,
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    let reset = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        error.set(None);
        #[derive(Serialize)]
        struct A {
            recovery: String,
            new: String,
        }
        let a = A {
            recovery: recovery.get_untracked(),
            new: new_pw.get_untracked(),
        };
        spawn_local(async move {
            match ipc::invoke::<String>("reset_password", a).await {
                Ok(k) => new_key.set(Some(k)),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let name = move || c.state.get().map(|s| s.server_name).unwrap_or_default();
    view! {
        <div class="gate">
            {hero("The control panel is locked.", "SpareLine Server keeps running while the panel is locked. Unlock it to manage people, invites and announcements, or to check on the server.")}
            <section class="gate-form" aria-label="Unlock">
                {move || if let Some(k) = new_key.get() {
                    view! {
                        <div class="stack">
                            <h2>"Password reset"</h2>
                            <p class="muted">"Your old recovery key no longer works. Here is the new one. Store it safely."</p>
                            <div class="code-block">{k}</div>
                            <button class="btn wide" on:click=move |_| spawn_local(refresh())>"Open the control panel"</button>
                        </div>
                    }.into_any()
                } else if resetting.get() {
                    view! {
                        <form class="stack" on:submit=reset novalidate>
                            <h2>"Reset the admin password"</h2>
                            <div class="field">
                                <label for="u-rk">"Recovery key"</label>
                                <input id="u-rk" class="input code" placeholder="RK-XXXX-XXXX-…" prop:value=move || recovery.get() on:input=move |e| recovery.set(event_target_value(&e))/>
                            </div>
                            <div class="field">
                                <label for="u-np">"New admin password"</label>
                                <input id="u-np" class="input" type="password" autocomplete="new-password" prop:value=move || new_pw.get() on:input=move |e| new_pw.set(event_target_value(&e))/>
                            </div>
                            <Show when=move || error.get().is_some()>
                                <div class="notice fail" role="alert"><Icon name="alert"/><span>{move || error.get().unwrap_or_default()}</span></div>
                            </Show>
                            <button type="submit" class="btn wide">"Reset password"</button>
                            <button type="button" class="btn ghost" on:click=move |_| { resetting.set(false); error.set(None); }>"Back"</button>
                        </form>
                    }.into_any()
                } else {
                    view! {
                        <form class="stack" on:submit=submit novalidate>
                            <h2>{move || format!("Unlock {}", name())}</h2>
                            <div class="field">
                                <label for="u-pw">"Admin password"</label>
                                <input id="u-pw" class="input" type="password" autocomplete="current-password" autofocus prop:value=move || pw.get() on:input=move |e| pw.set(event_target_value(&e))/>
                            </div>
                            <Show when=move || error.get().is_some()>
                                <div class="notice fail" role="alert"><Icon name="alert"/><span>{move || error.get().unwrap_or_default()}</span></div>
                            </Show>
                            <button type="submit" class="btn wide" disabled=move || busy.get()>
                                {move || if busy.get() { view! { <Spinner/> "Unlocking…" }.into_any() } else { "Unlock".into_any() }}
                            </button>
                            <button type="button" class="btn ghost" on:click=move |_| resetting.set(true)>"Forgot the password? Use the recovery key"</button>
                        </form>
                    }.into_any()
                }}
            </section>
        </div>
    }
}
