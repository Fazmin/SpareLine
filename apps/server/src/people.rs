//! People: accounts, their groups and devices, and connection troubleshooting.
//! Conversations are not visible here (they are end-to-end encrypted).

use std::collections::BTreeSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::panel::*;
use sl_proto::{GroupId, UserId};
use sl_ui::{Avatar, Icon, Modal, format, ipc, toasts};

use crate::ctx::{Page, act, call, call0, ctx};

pub fn event_tag(k: ConnEventKind) -> impl IntoView {
    let (class, label) = match k {
        ConnEventKind::Connected => ("tag ok", "Connected"),
        ConnEventKind::Registered => ("tag ok", "Joined"),
        ConnEventKind::Disconnected => ("tag", "Disconnected"),
        ConnEventKind::TcpAccepted => ("tag", "Reached server"),
        ConnEventKind::TlsFailed => ("tag fail", "Secure connection failed"),
        ConnEventKind::AuthFailed => ("tag fail", "Sign-in failed"),
        ConnEventKind::ProtocolMismatch => ("tag warn", "Version mismatch"),
        ConnEventKind::RateLimited => ("tag warn", "Too many attempts"),
        ConnEventKind::SessionRevoked => ("tag warn", "Signed out"),
    };
    view! { <span class=class>{label}</span> }
}

#[derive(Serialize)]
struct Id {
    id: UserId,
}

#[component]
pub fn PeoplePage() -> impl IntoView {
    let c = ctx();
    let users = RwSignal::new(None::<Vec<UserRow>>);
    let groups = RwSignal::new(Vec::<GroupRow>::new());
    let q = RwSignal::new(String::new());
    let gfilter = RwSignal::new(String::new());
    let sfilter = RwSignal::new(String::new());
    let selected = RwSignal::new(c.focus_user.get_untracked());
    c.focus_user.set(None);
    let load = move || {
        spawn_local(async move {
            if let Ok(u) = call0::<Vec<UserRow>>("users").await {
                users.set(Some(u));
            }
            if let Ok(g) = call0::<Vec<GroupRow>>("groups").await {
                groups.set(g);
            }
        });
    };
    load();
    let reload = Callback::new(move |_| load());
    let rows = move || {
        let query = q.get().to_lowercase();
        let g: Option<GroupId> = gfilter.get().parse().ok();
        let st = sfilter.get();
        users
            .get()
            .unwrap_or_default()
            .into_iter()
            .filter(|u| {
                (query.is_empty()
                    || u.display_name.to_lowercase().contains(&query)
                    || u.username.contains(&query))
                    && g.is_none_or(|g| u.groups.iter().any(|x| x.id == g))
                    && match st.as_str() {
                        "online" => u.online,
                        "disabled" => u.status == UserStatus::Disabled,
                        "never" => u.devices == 0,
                        _ => true,
                    }
            })
            .collect::<Vec<_>>()
    };
    view! {
        <div class="page-head">
            <div class="grow">
                <h1>"People"</h1>
                <p class="lede">"Everyone with a SpareLine account. People join with an invite; you decide which groups they're in, and therefore who they can see and message."</p>
            </div>
            <button class="btn" on:click=move |_| { c.invite_now.set(true); c.page.set(Page::Invites); }><Icon name="invite"/>"Invite people"</button>
        </div>
        <div class="toolbar" role="search">
            <input class="input" type="search" aria-label="Search people" placeholder="Search by name or username" prop:value=move || q.get() on:input=move |e| q.set(event_target_value(&e))/>
            <select class="select" aria-label="Filter by group" on:change=move |e| gfilter.set(event_target_value(&e))>
                <option value="">"All groups"</option>
                {move || groups.get().into_iter().map(|g| view! { <option value=g.id.to_string()>{g.name}</option> }).collect_view()}
            </select>
            <select class="select" aria-label="Filter by status" on:change=move |e| sfilter.set(event_target_value(&e))>
                <option value="">"Everyone"</option>
                <option value="online">"Online now"</option>
                <option value="never">"No device yet"</option>
                <option value="disabled">"Disabled"</option>
            </select>
            <span class="grow"></span>
            <span class="hint">{move || format::plural(rows().len() as u64, "person", "people")}</span>
        </div>
        <div class="split">
            <div class="box flush">
                {move || match users.get() {
                    None => view! { <div class="empty"><sl_ui::Spinner label="Loading people"/></div> }.into_any(),
                    Some(all) if all.is_empty() => view! {
                        <div class="empty">
                            <h2>"No one has joined yet"</h2>
                            <p>"Create invites and send them to staff. Each invite works once."</p>
                            <button class="btn" on:click=move |_| { c.invite_now.set(true); c.page.set(Page::Invites); }>"Create invites"</button>
                        </div>
                    }.into_any(),
                    Some(_) => view! {
                        <table class="table">
                            <thead><tr><th scope="col">"Name"</th><th scope="col">"Groups"</th><th scope="col">"Devices"</th><th scope="col">"Status"</th></tr></thead>
                            <tbody>
                                <For each=rows key=|u| (u.id, u.online, u.status == UserStatus::Disabled, u.groups.len(), u.devices) let:u>
                                    {
                                        let id = u.id;
                                        view! {
                                            <tr class="clickable" aria-selected=move || (selected.get() == Some(id)).to_string() on:click=move |_| selected.set(Some(id))>
                                                <td>
                                                    <button class="who-cell" style="border:0;background:none;padding:0;cursor:pointer;text-align:left" on:click=move |_| selected.set(Some(id))>
                                                        <Avatar name=Signal::stored(u.display_name.clone()) seed=Signal::stored(u.id.to_string()) size=2.0/>
                                                        <span><strong>{u.display_name.clone()}</strong><small>{format!("@{}", u.username)}</small></span>
                                                    </button>
                                                </td>
                                                <td><div class="tags">{u.groups.iter().map(|g| view! { <span class="tag">{g.name.clone()}</span> }).collect_view()}{u.groups.is_empty().then(|| view! { <span class="hint">"None"</span> })}</div></td>
                                                <td>{u.devices}</td>
                                                <td>{if u.status == UserStatus::Disabled {
                                                    view! { <span class="tag fail">"Disabled"</span> }.into_any()
                                                } else if u.online {
                                                    view! { <span class="tag ok"><span class="presence" data-p="online" style="border-width:0"></span>"Online"</span> }.into_any()
                                                } else {
                                                    view! { <span class="hint">{u.last_seen.map(|t| format!("Seen {}", format::ago(t))).unwrap_or_else(|| "Never connected".into())}</span> }.into_any()
                                                }}</td>
                                            </tr>
                                        }
                                    }
                                </For>
                            </tbody>
                        </table>
                    }.into_any(),
                }}
            </div>
            {move || match selected.get() {
                Some(id) => view! { <UserDrawer id=id groups=groups on_change=reload on_close=Callback::new(move |_| selected.set(None))/> }.into_any(),
                None => view! {
                    <aside class="drawer">
                        <p class="muted">"Select someone to see their groups, devices and connection history."</p>
                        <p class="privacy-note"><Icon name="shield"/><span>"Conversations are end-to-end encrypted. You can manage accounts here, but you can't read anyone's messages."</span></p>
                    </aside>
                }.into_any(),
            }}
        </div>
    }
}

#[component]
fn UserDrawer(
    id: UserId,
    groups: RwSignal<Vec<GroupRow>>,
    on_change: Callback<()>,
    on_close: Callback<()>,
) -> impl IntoView {
    let detail = RwSignal::new(None::<UserDetail>);
    let picked = RwSignal::new(BTreeSet::<GroupId>::new());
    let code = RwSignal::new(None::<CreatedInvite>);
    let code_open = RwSignal::new(false);
    let confirm_delete = RwSignal::new(false);
    // Refresh quietly: only redraw when something changed, and only reset the group
    // checkboxes on first load or after a save (so unsaved edits survive).
    let load = move |reset_groups: bool| {
        spawn_local(async move {
            #[derive(Serialize)]
            struct A {
                id: UserId,
            }
            if let Ok(d) = call::<UserDetail>("user_detail", A { id }).await {
                if reset_groups || detail.get_untracked().is_none() {
                    picked.set(d.user.groups.iter().map(|g| g.id).collect());
                }
                if detail.get_untracked().as_ref() != Some(&d) {
                    detail.set(Some(d));
                }
            }
        });
    };
    load(true);
    if let Ok(h) = set_interval_with_handle(move || load(false), std::time::Duration::from_secs(5))
    {
        on_cleanup(move || h.clear());
    }
    let changed = move || {
        load(true);
        on_change.run(());
    };
    let save_groups = move |_| {
        #[derive(Serialize)]
        struct A {
            id: UserId,
            groups: Vec<GroupId>,
        }
        act(
            "set_user_groups",
            A {
                id,
                groups: picked.get_untracked().into_iter().collect(),
            },
            "Groups updated.",
            changed,
        );
    };
    let setup_code = move |_| {
        #[derive(Serialize)]
        struct A {
            id: UserId,
            expires: i64,
        }
        spawn_local(async move {
            match call::<CreatedInvite>(
                "create_setup_code",
                A {
                    id,
                    expires: 86_400,
                },
            )
            .await
            {
                Ok(c) => {
                    code.set(Some(c));
                    code_open.set(true);
                }
                Err(e) => toasts().error(e.message),
            }
        });
    };
    let copy = move |text: String| {
        spawn_local(async move {
            if ipc::copy_text(&text).await {
                toasts().info("Copied.");
            }
        });
    };
    view! {
        <aside class="drawer" aria-label="Person details">
            {move || detail.get().map(|d| {
                let u = d.user.clone();
                let disabled = u.status == UserStatus::Disabled;
                view! {
                    <div class="row" style="align-items:flex-start">
                        <Avatar name=Signal::stored(u.display_name.clone()) seed=Signal::stored(u.id.to_string()) size=3.0/>
                        <div class="grow">
                            <h2>{u.display_name.clone()}</h2>
                            <p class="muted">{format!("@{}, joined {}", u.username, format::full(u.created_at))}</p>
                        </div>
                        <button class="icon-btn" aria-label="Close" on:click=move |_| on_close.run(())><Icon name="close"/></button>
                    </div>

                    <h3>"Groups"</h3>
                    <p class="hint">"They can see and message people in these groups."</p>
                    <ul class="checklist" style="margin-top:var(--s2)">
                        {move || groups.get().into_iter().map(|g| {
                            let gid = g.id;
                            view! {
                                <li><label>
                                    <input type="checkbox" prop:checked=move || picked.get().contains(&gid) on:change=move |e| {
                                        let on = event_target_checked(&e);
                                        picked.update(|p| { if on { p.insert(gid); } else { p.remove(&gid); } });
                                    }/>
                                    {g.name}
                                </label></li>
                            }
                        }).collect_view()}
                    </ul>
                    <button class="btn quiet small" style="margin-top:var(--s2)" on:click=save_groups>"Save groups"</button>

                    <h3>"Devices"</h3>
                    {if d.devices.is_empty() {
                        view! { <p class="hint">"No devices. They haven't finished joining."</p> }.into_any()
                    } else {
                        view! {
                            <ul class="mini-list">
                                {d.devices.clone().into_iter().map(|dv| {
                                    let did = dv.id;
                                    view! {
                                        <li>
                                            <Icon name="server"/>
                                            <span class="grow">
                                                <strong>{dv.name.clone()}</strong>
                                                {dv.online.then(|| view! { " " <span class="tag ok">"Online"</span> })}
                                                {dv.revoked.then(|| view! { " " <span class="tag">"Removed"</span> })}
                                                <span class="hint" style="display:block">{format!("{} {}, last seen {}", dv.platform, dv.client_version, dv.last_seen.map(format::ago).unwrap_or_else(|| "never".into()))}</span>
                                            </span>
                                            {(!dv.revoked).then(|| view! {
                                                <button class="btn ghost small" on:click=move |_| {
                                                    #[derive(Serialize)] struct A { id: uuid::Uuid }
                                                    act("revoke_device", A { id: did }, "Device removed and signed out.", changed);
                                                }>"Remove"</button>
                                            })}
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_any()
                    }}

                    <h3>"Sign-ins"</h3>
                    {if d.sessions.is_empty() {
                        view! { <p class="hint">"Not signed in anywhere."</p> }.into_any()
                    } else {
                        view! {
                            <ul class="mini-list">
                                {d.sessions.clone().into_iter().map(|s| {
                                    let sid = s.id.clone();
                                    view! {
                                        <li>
                                            <span class="grow">
                                                <span class="mono">{s.ip.clone()}</span>
                                                <span class="hint" style="display:block">{format!("Last used {}, expires {}", format::ago(s.last_used), format::until(s.expires_at))}</span>
                                            </span>
                                            <button class="btn ghost small" on:click=move |_| {
                                                #[derive(Serialize)] struct A { id: String }
                                                act("revoke_session", A { id: sid.clone() }, "Signed out.", changed);
                                            }>"Sign out"</button>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_any()
                    }}

                    <h3>"Connection history"</h3>
                    <p class="hint">"Use this when someone says SpareLine won't connect."</p>
                    {if d.recent_events.is_empty() {
                        view! { <p class="hint">"Nothing recorded in the last week."</p> }.into_any()
                    } else {
                        view! {
                            <ul class="mini-list">
                                {d.recent_events.clone().into_iter().take(25).map(|e| view! {
                                    <li style="align-items:flex-start">
                                        <span class="event-kind">{event_tag(e.kind)}</span>
                                        <span class="grow small">{e.detail.clone()}<span class="hint" style="display:block">{format!("{}, from {}", format::full(e.ts), e.ip)}</span></span>
                                    </li>
                                }).collect_view()}
                            </ul>
                        }.into_any()
                    }}
                    {(!d.health_reports.is_empty()).then(|| view! {
                        <h3>"Problems their app reported"</h3>
                        <ul class="mini-list">
                            {d.health_reports.clone().into_iter().take(5).map(|r| view! {
                                <li style="align-items:flex-start">
                                    <Icon name="alert"/>
                                    <span class="grow small">
                                        {r.report.failures.iter().map(|(k, n)| format!("{} ×{n}", k.replace('_', " "))).collect::<Vec<_>>().join(", ")}
                                        <span class="hint" style="display:block">{format!("Between {} and {}. Last error: {}", format::time(r.report.window_start), format::time(r.report.window_end), r.report.last_error)}</span>
                                    </span>
                                </li>
                            }).collect_view()}
                        </ul>
                    })}

                    <h3>"Account"</h3>
                    <div class="stack" style="gap:var(--s2)">
                        <button class="btn quiet" on:click=setup_code><Icon name="key"/>"Create a setup code"</button>
                        <span class="hint">"For a new computer or a forgotten password. Valid for 24 hours; it signs out their other devices."</span>
                        <button class="btn quiet" on:click=move |_| act("sign_out_everywhere", Id { id }, "Signed out everywhere.", changed)><Icon name="logout"/>"Sign out everywhere"</button>
                        <button class="btn quiet" on:click=move |_| {
                            #[derive(Serialize)] struct A { id: UserId, disabled: bool }
                            act("set_user_disabled", A { id, disabled: !disabled }, if disabled { "Account enabled." } else { "Account disabled." }, changed);
                        }>{if disabled { "Enable account" } else { "Disable account" }}</button>
                        <button class="btn ghost" style="color:var(--fail)" on:click=move |_| confirm_delete.set(true)><Icon name="trash"/>"Delete account"</button>
                    </div>
                    <p class="privacy-note"><Icon name="shield"/><span>"Their conversations are end-to-end encrypted and can't be viewed here."</span></p>
                }
            })}
            <Modal open=code_open title="Setup code">
                {move || code.get().map(|c| {
                    let msg = c.message.clone();
                    let cd = c.code.clone();
                    view! {
                        <div class="modal-body stack">
                            <p>{format!("Give this to {}. It works once and expires in 24 hours.", c.row.target_user.map(|u| u.display_name).unwrap_or_default())}</p>
                            <div class="code-block">{c.code.clone()}</div>
                            <details><summary>"Message to send"</summary><pre class="small" style="white-space:pre-wrap">{c.message.clone()}</pre></details>
                        </div>
                        <div class="modal-foot">
                            <button class="btn quiet" on:click=move |_| copy(cd.clone())><Icon name="copy"/>"Copy code"</button>
                            <button class="btn" on:click=move |_| copy(msg.clone())><Icon name="copy"/>"Copy message"</button>
                        </div>
                    }
                })}
            </Modal>
            <Modal open=confirm_delete title="Delete this account?">
                <div class="modal-body"><p>"They're removed from every chat and signed out. Messages already on their colleagues' devices stay there. This can't be undone; disabling is reversible."</p></div>
                <div class="modal-foot">
                    <button class="btn quiet" on:click=move |_| confirm_delete.set(false)>"Cancel"</button>
                    <button class="btn danger" on:click=move |_| {
                        confirm_delete.set(false);
                        act("delete_user", Id { id }, "Account deleted.", move || { on_change.run(()); on_close.run(()); });
                    }>"Delete account"</button>
                </div>
            </Modal>
        </aside>
    }
}
