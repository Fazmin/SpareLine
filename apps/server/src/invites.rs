//! Invites: create single-use codes, hand them out, track them.

use std::collections::BTreeSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::GroupId;
use sl_proto::panel::*;
use sl_ui::{Icon, Modal, format, ipc, toasts};

use crate::ctx::{act, call, call0, ctx};

fn status_tag(s: InviteStatus) -> impl IntoView {
    let (c, l) = match s {
        InviteStatus::Pending => ("tag ok", "Waiting"),
        InviteStatus::Used => ("tag", "Used"),
        InviteStatus::Expired => ("tag warn", "Expired"),
        InviteStatus::Revoked => ("tag", "Cancelled"),
    };
    view! { <span class=c>{l}</span> }
}

fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn cards_html(created: &[CreatedInvite], server: &str) -> String {
    let mut cards = String::new();
    for c in created {
        let esc = |s: &str| {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        cards.push_str(&format!(
            "<section class=card><h2>SpareLine invite</h2><p>Server: <b>{}</b></p><p class=code>{}</p><p>Open SpareLine, choose <b>I have an invite</b> and enter this code. It works once and expires {}.</p><p class=small>{}</p></section>",
            esc(server),
            esc(&c.code),
            esc(&format::full(c.row.expires_at)),
            esc(&c.row.groups.iter().map(|g| g.name.clone()).collect::<Vec<_>>().join(", ")),
        ));
    }
    format!(
        "<!doctype html><meta charset=utf-8><title>SpareLine invites</title><style>body{{font-family:'Atkinson Hyperlegible Next','Segoe UI',sans-serif;margin:2rem}}.card{{border:1px dashed #555;border-radius:10px;padding:1.2rem 1.5rem;margin:0 0 1rem;break-inside:avoid;max-width:34rem}}.code{{font-family:Consolas,monospace;font-size:1.6rem;letter-spacing:.08em}}.small{{color:#555;font-size:.85rem}}</style>{cards}"
    )
}

#[component]
pub fn InvitesPage() -> impl IntoView {
    let c = ctx();
    let invites = RwSignal::new(None::<Vec<InviteRow>>);
    let groups = RwSignal::new(Vec::<GroupRow>::new());
    let picked = RwSignal::new(BTreeSet::<GroupId>::new());
    let count = RwSignal::new(1u32);
    let expires = RwSignal::new(7 * 86_400i64);
    let note = RwSignal::new(String::new());
    let created = RwSignal::new(Vec::<CreatedInvite>::new());
    let created_open = RwSignal::new(false);
    let show_all = RwSignal::new(false);
    let form_open = RwSignal::new(c.invite_now.get_untracked());
    c.invite_now.set(false);
    let load = move || {
        spawn_local(async move {
            if let Ok(i) = call0::<Vec<InviteRow>>("invites").await {
                invites.set(Some(i));
            }
            if let Ok(g) = call0::<Vec<GroupRow>>("groups").await {
                groups.set(g);
            }
        });
    };
    load();
    let create = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let input = CreateInvitesInput {
            count: count.get_untracked().clamp(1, 200),
            group_ids: picked.get_untracked().into_iter().collect(),
            expires_in_secs: expires.get_untracked(),
            note: note.get_untracked(),
        };
        if input.group_ids.is_empty() {
            toasts()
                .warn("Choose at least one group, or the new person won't be able to find anyone.");
            return;
        }
        #[derive(Serialize)]
        struct A {
            input: CreateInvitesInput,
        }
        spawn_local(async move {
            match call::<Vec<CreatedInvite>>("create_invites", A { input }).await {
                Ok(list) => {
                    created.set(list);
                    created_open.set(true);
                    form_open.set(false);
                    note.set(String::new());
                    load();
                }
                Err(e) => toasts().error(e.message),
            }
        });
    };
    let copy = move |text: String, what: &'static str| {
        spawn_local(async move {
            if ipc::copy_text(&text).await {
                toasts().info(format!("{what} copied."));
            }
        });
    };
    let save = move |filename: &'static str, content: String| {
        #[derive(Serialize)]
        struct A {
            filename: String,
            content: String,
        }
        spawn_local(async move {
            match call::<Option<String>>(
                "save_text",
                A {
                    filename: filename.into(),
                    content,
                },
            )
            .await
            {
                Ok(Some(p)) => toasts().info(format!("Saved to {p}")),
                Ok(None) => {}
                Err(e) => toasts().error(e.message),
            }
        });
    };
    let server_name = move || {
        c.overview
            .get()
            .map(|o| o.o.server_name)
            .unwrap_or_default()
    };
    let rows = move || {
        let all = show_all.get();
        invites
            .get()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| all || i.status == InviteStatus::Pending)
            .collect::<Vec<_>>()
    };
    view! {
        <div class="page-head">
            <div class="grow">
                <h1>"Invites"</h1>
                <p class="lede">"Each invite is a single-use code. Hand them out before you need them, so everyone is ready before the next outage. Codes are shown only once; the server keeps a fingerprint of each, not the code itself."</p>
            </div>
            <button class="btn" on:click=move |_| form_open.update(|o| *o = !*o) aria-expanded=move || form_open.get().to_string()><Icon name="plus"/>"Create invites"</button>
        </div>
        <Show when=move || form_open.get()>
            <form class="box stack" style="margin-bottom:var(--s5)" on:submit=create>
                <h2 style="font-size:var(--t-lg)">"New invites"</h2>
                <fieldset style="border:0;padding:0;margin:0">
                    <legend class="label">"Groups they'll join"</legend>
                    <ul class="checklist" style="margin-top:var(--s2)">
                        {move || groups.get().into_iter().map(|g| {
                            let gid = g.id;
                            view! { <li><label>
                                <input type="checkbox" prop:checked=move || picked.get().contains(&gid) on:change=move |e| {
                                    let on = event_target_checked(&e);
                                    picked.update(|p| { if on { p.insert(gid); } else { p.remove(&gid); } });
                                }/>
                                {g.name}
                            </label></li> }
                        }).collect_view()}
                    </ul>
                    <Show when=move || groups.get().is_empty()>
                        <p class="hint">"Create a group first on the Groups page."</p>
                    </Show>
                </fieldset>
                <div class="row wrap" style="align-items:flex-end">
                    <div class="field">
                        <label for="i-count">"How many"</label>
                        <input id="i-count" class="input" type="number" min="1" max="200" style="width:7rem" prop:value=move || count.get().to_string() on:input=move |e| count.set(event_target_value(&e).parse().unwrap_or(1))/>
                    </div>
                    <div class="field">
                        <label for="i-exp">"Valid for"</label>
                        <select id="i-exp" class="select" on:change=move |e| expires.set(event_target_value(&e).parse().unwrap_or(604_800))>
                            <option value="3600">"1 hour"</option>
                            <option value="86400">"1 day"</option>
                            <option value="259200">"3 days"</option>
                            <option value="604800" selected>"7 days"</option>
                            <option value="2592000">"30 days"</option>
                        </select>
                    </div>
                    <div class="field grow">
                        <label for="i-note">"Note for yourself (optional)"</label>
                        <input id="i-note" class="input" placeholder="For the London office" prop:value=move || note.get() on:input=move |e| note.set(event_target_value(&e))/>
                    </div>
                </div>
                <div class="row"><button class="btn" type="submit">"Create"</button><button type="button" class="btn ghost" on:click=move |_| form_open.set(false)>"Cancel"</button></div>
            </form>
        </Show>
        <div class="toolbar">
            <label class="row" style="cursor:pointer">
                <input type="checkbox" style="width:1.1rem;height:1.1rem;accent-color:var(--signal)" on:change=move |e| show_all.set(event_target_checked(&e))/>
                "Show used, expired and cancelled"
            </label>
        </div>
        <div class="box flush">
            {move || {
                let r = rows();
                if invites.get().is_none() {
                    view! { <div class="empty"><sl_ui::Spinner/></div> }.into_any()
                } else if r.is_empty() {
                    view! { <div class="empty"><h2>"No open invites"</h2><p>"Create invites and send them to staff by email or print them out."</p></div> }.into_any()
                } else {
                    view! {
                        <table class="table">
                            <thead><tr><th scope="col">"Status"</th><th scope="col">"For"</th><th scope="col">"Groups"</th><th scope="col">"Created"</th><th scope="col">"Expires"</th><th scope="col"><span class="sr-only">"Actions"</span></th></tr></thead>
                            <tbody>
                                {r.into_iter().map(|i| {
                                    let id = i.id;
                                    let who = match (&i.used_by, &i.target_user, i.kind) {
                                        (Some(u), _, _) => format!("Used by {}", u.display_name),
                                        (None, Some(u), InviteKind::Setup) => format!("Setup code for {}", u.display_name),
                                        _ => if i.note.is_empty() { "New person".to_string() } else { i.note.clone() },
                                    };
                                    view! {
                                        <tr>
                                            <td>{status_tag(i.status)}</td>
                                            <td>{who}</td>
                                            <td><div class="tags">{i.groups.iter().map(|g| view! { <span class="tag">{g.name.clone()}</span> }).collect_view()}</div></td>
                                            <td class="hint">{format::short_when(i.created_at)}</td>
                                            <td class="hint">{if i.status == InviteStatus::Pending { format::until(i.expires_at) } else { format::short_when(i.expires_at) }}</td>
                                            <td>{(i.status == InviteStatus::Pending).then(|| view! {
                                                <button class="btn ghost small" on:click=move |_| {
                                                    #[derive(Serialize)] struct A { id: uuid::Uuid }
                                                    act("revoke_invite", A { id }, "Invite cancelled.", load);
                                                }>"Cancel"</button>
                                            })}</td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_any()
                }
            }}
        </div>
        <Modal open=created_open title="Your new invites" wide=true>
            <div class="modal-body">
                <p class="notice warn" style="margin-bottom:var(--s4)"><Icon name="info"/><span>"Copy or save these now. For security, codes can't be shown again."</span></p>
                {move || created.get().into_iter().map(|c| {
                    let msg = c.message.clone();
                    let code = c.code.clone();
                    view! {
                        <div class="code-card">
                            <div class="row wrap">
                                <span class="code grow">{c.code.clone()}</span>
                                <button class="btn quiet small" on:click=move |_| copy(code.clone(), "Code")><Icon name="copy"/>"Copy code"</button>
                                <button class="btn small" on:click=move |_| copy(msg.clone(), "Invite message")><Icon name="copy"/>"Copy invite message"</button>
                            </div>
                        </div>
                    }
                }).collect_view()}
            </div>
            <div class="modal-foot">
                <button class="btn quiet" on:click=move |_| {
                    let mut csv = String::from("code,groups,expires,link\n");
                    for c in created.get_untracked() {
                        csv.push_str(&format!("{},{},{},{}\n", c.code, csv_escape(&c.row.groups.iter().map(|g| g.name.clone()).collect::<Vec<_>>().join("; ")), csv_escape(&format::full(c.row.expires_at)), c.link));
                    }
                    save("spareline-invites.csv", csv);
                }><Icon name="download"/>"Save as CSV"</button>
                <button class="btn quiet" on:click=move |_| save("spareline-invite-cards.html", cards_html(&created.get_untracked(), &server_name()))><Icon name="printer"/>"Save printable cards"</button>
                <button class="btn" on:click=move |_| {
                    let all = created.get_untracked().into_iter().map(|c| c.message).collect::<Vec<_>>().join("\n\n----------\n\n");
                    copy(all, "All invite messages");
                }><Icon name="copy"/>"Copy all messages"</button>
            </div>
        </Modal>
    }
}
