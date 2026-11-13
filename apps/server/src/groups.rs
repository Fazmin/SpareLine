//! Groups: who can see and message whom.

use std::collections::BTreeSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::panel::*;
use sl_proto::{GroupId, UserId};
use sl_ui::{Avatar, Icon, Modal, format, toasts};

use crate::ctx::{act, call, call0};

#[component]
pub fn GroupsPage() -> impl IntoView {
    let groups = RwSignal::new(None::<Vec<GroupRow>>);
    let selected = RwSignal::new(None::<GroupId>);
    let name = RwSignal::new(String::new());
    let load = move || {
        spawn_local(async move {
            if let Ok(g) = call0::<Vec<GroupRow>>("groups").await {
                if selected.get_untracked().is_none() {
                    selected.set(g.first().map(|x| x.id));
                }
                groups.set(Some(g));
            }
        });
    };
    load();
    let create = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let n = name.get_untracked();
        if n.trim().is_empty() {
            return;
        }
        #[derive(Serialize)]
        struct A {
            name: String,
            description: String,
        }
        spawn_local(async move {
            match call::<GroupRow>(
                "create_group",
                A {
                    name: n,
                    description: String::new(),
                },
            )
            .await
            {
                Ok(g) => {
                    name.set(String::new());
                    selected.set(Some(g.id));
                    toasts().info(format!("Group \"{}\" created.", g.name));
                    load();
                }
                Err(e) => toasts().error(e.message),
            }
        });
    };
    view! {
        <div class="page-head">
            <div class="grow">
                <h1>"Groups"</h1>
                <p class="lede">"Groups decide who can find whom. People only see colleagues who share a group with them, and group chats belong to one group."</p>
            </div>
        </div>
        <form class="toolbar" on:submit=create>
            <input class="input" aria-label="New group name" placeholder="New group, for example Finance" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/>
            <button class="btn" type="submit"><Icon name="plus"/>"Create group"</button>
        </form>
        <div class="split">
            <div class="box flush">
                {move || match groups.get() {
                    None => view! { <div class="empty"><sl_ui::Spinner/></div> }.into_any(),
                    Some(g) if g.is_empty() => view! {
                        <div class="empty"><h2>"No groups yet"</h2><p>"Create one per team or site. Someone can be in several groups."</p></div>
                    }.into_any(),
                    Some(g) => view! {
                        <table class="table">
                            <thead><tr><th scope="col">"Group"</th><th scope="col">"Members"</th><th scope="col">"Created"</th></tr></thead>
                            <tbody>
                                {g.into_iter().map(|x| {
                                    let id = x.id;
                                    view! {
                                        <tr class="clickable" aria-selected=move || (selected.get() == Some(id)).to_string() on:click=move |_| selected.set(Some(id))>
                                            <td><button style="border:0;background:none;padding:0;cursor:pointer;font-weight:650;text-align:left" on:click=move |_| selected.set(Some(id))>{x.name.clone()}</button>
                                                {(!x.description.is_empty()).then(|| view! { <span class="hint" style="display:block">{x.description.clone()}</span> })}</td>
                                            <td>{x.member_count}</td>
                                            <td class="hint">{format::short_when(x.created_at)}</td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_any(),
                }}
            </div>
            {move || selected.get().map(|id| view! { <GroupDrawer id=id on_change=Callback::new(move |_| load()) on_deleted=Callback::new(move |_| { selected.set(None); load(); })/> })}
        </div>
    }
}

#[component]
fn GroupDrawer(id: GroupId, on_change: Callback<()>, on_deleted: Callback<()>) -> impl IntoView {
    let detail = RwSignal::new(None::<GroupDetail>);
    let everyone = RwSignal::new(Vec::<UserRow>::new());
    let members = RwSignal::new(BTreeSet::<UserId>::new());
    let name = RwSignal::new(String::new());
    let desc = RwSignal::new(String::new());
    let q = RwSignal::new(String::new());
    let confirm = RwSignal::new(false);
    let load = move || {
        spawn_local(async move {
            #[derive(Serialize)]
            struct A {
                id: GroupId,
            }
            if let Ok(d) = call::<GroupDetail>("group_detail", A { id }).await {
                members.set(d.members.iter().map(|m| m.id).collect());
                name.set(d.group.name.clone());
                desc.set(d.group.description.clone());
                detail.set(Some(d));
            }
            if let Ok(u) = call0::<Vec<UserRow>>("users").await {
                everyone.set(u);
            }
        });
    };
    load();
    let changed = move || {
        load();
        on_change.run(());
    };
    let save = move |_| {
        #[derive(Serialize)]
        struct A {
            id: GroupId,
            name: String,
            description: String,
        }
        act(
            "update_group",
            A {
                id,
                name: name.get_untracked(),
                description: desc.get_untracked(),
            },
            "Group saved.",
            changed,
        );
    };
    let save_members = move |_| {
        #[derive(Serialize)]
        struct A {
            id: GroupId,
            users: Vec<UserId>,
        }
        act(
            "set_group_members",
            A {
                id,
                users: members.get_untracked().into_iter().collect(),
            },
            "Members updated.",
            changed,
        );
    };
    let original = move || {
        detail
            .get()
            .map(|d| d.members.iter().map(|m| m.id).collect::<BTreeSet<_>>())
            .unwrap_or_default()
    };
    let dirty = move || original() != members.get();
    view! {
        <aside class="drawer" aria-label="Group details">
            <div class="field">
                <label for="g-name">"Name"</label>
                <input id="g-name" class="input" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/>
            </div>
            <div class="field" style="margin-top:var(--s3)">
                <label for="g-desc">"Description (optional)"</label>
                <input id="g-desc" class="input" prop:value=move || desc.get() on:input=move |e| desc.set(event_target_value(&e))/>
            </div>
            <button class="btn quiet small" style="margin-top:var(--s3)" on:click=save>"Save name"</button>

            <h3>{move || format!("Members ({})", members.get().len())}</h3>
            <input class="input" type="search" aria-label="Find people" placeholder="Find people" prop:value=move || q.get() on:input=move |e| q.set(event_target_value(&e))/>
            <ul class="mini-list" style="margin-top:var(--s2);max-height:22rem;overflow:auto">
                {move || {
                    let query = q.get().to_lowercase();
                    let mut list = everyone.get();
                    list.sort_by_key(|u| (!members.get().contains(&u.id), u.display_name.to_lowercase()));
                    list.into_iter().filter(|u| query.is_empty() || u.display_name.to_lowercase().contains(&query) || u.username.contains(&query)).map(|u| {
                        let uid = u.id;
                        view! {
                            <li>
                                <label class="row grow" style="cursor:pointer">
                                    <input type="checkbox" style="width:1.1rem;height:1.1rem;accent-color:var(--signal)" prop:checked=move || members.get().contains(&uid) on:change=move |e| {
                                        let on = event_target_checked(&e);
                                        members.update(|m| { if on { m.insert(uid); } else { m.remove(&uid); } });
                                    }/>
                                    <Avatar name=Signal::stored(u.display_name.clone()) seed=Signal::stored(u.id.to_string()) size=1.8/>
                                    <span class="grow">{u.display_name.clone()}<span class="hint" style="display:block">{format!("@{}", u.username)}</span></span>
                                </label>
                            </li>
                        }
                    }).collect_view()
                }}
            </ul>
            <div class="row" style="margin-top:var(--s3)">
                <button class="btn" disabled=move || !dirty() on:click=save_members>"Save members"</button>
                <Show when=dirty><span class="hint">"Unsaved changes"</span></Show>
            </div>
            <p class="hint" style="margin-top:var(--s2)">"Removing someone takes them out of this group's group chats straight away."</p>

            <h3>"Delete group"</h3>
            <button class="btn ghost" style="color:var(--fail)" on:click=move |_| confirm.set(true)><Icon name="trash"/>"Delete this group"</button>
            <Modal open=confirm title="Delete this group?">
                <div class="modal-body"><p>"Members lose access to this group's group chats, and people who only shared this group can no longer message each other. Accounts are kept."</p></div>
                <div class="modal-foot">
                    <button class="btn quiet" on:click=move |_| confirm.set(false)>"Cancel"</button>
                    <button class="btn danger" on:click=move |_| {
                        confirm.set(false);
                        #[derive(Serialize)] struct A { id: GroupId }
                        act("delete_group", A { id }, "Group deleted.", move || on_deleted.run(()));
                    }>"Delete group"</button>
                </div>
            </Modal>
        </aside>
    }
}
