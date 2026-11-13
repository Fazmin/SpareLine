//! People: everyone you share a group with, organised by group.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::client_ipc::*;
use sl_proto::{ConversationKind, Directory, UserId};
use sl_ui::{Avatar, Icon, ipc, presence_label, toasts};

use crate::state::store;

#[component]
pub fn People() -> impl IntoView {
    let s = store();
    let q = RwSignal::new(String::new());
    spawn_local(async move {
        if let Ok(d) = ipc::invoke0::<Directory>("directory").await {
            s.directory.set(d);
        }
    });
    let message = move |u: UserId| {
        spawn_local(async move {
            #[derive(Serialize)]
            struct A {
                input: StartChatInput,
            }
            let input = StartChatInput {
                kind: ConversationKind::Direct,
                members: vec![u],
                scope_group: None,
                title: String::new(),
            };
            match ipc::invoke::<ConversationView>("start_chat", A { input }).await {
                Ok(c) => {
                    s.upsert_conversation(c.clone());
                    s.open(c.id);
                }
                Err(e) => toasts().error(e.message),
            }
        });
    };
    let sections = move || {
        let query = q.get().to_lowercase();
        s.directory.with(|d| {
            d.groups
                .iter()
                .map(|g| {
                    let users: Vec<_> = d
                        .users
                        .iter()
                        .filter(|u| u.groups.contains(&g.id))
                        .filter(|u| {
                            query.is_empty()
                                || u.display_name.to_lowercase().contains(&query)
                                || u.username.contains(&query)
                        })
                        .cloned()
                        .collect();
                    (g.clone(), users)
                })
                .collect::<Vec<_>>()
        })
    };
    view! {
        <main class="page" id="main" tabindex="-1">
            <crate::shell::Banners/>
            <div class="page-inner">
                <h1>"People"</h1>
                <p class="page-lede">"You can see and message people who share a group with you. Your admin manages groups."</p>
                <div class="searchbox" style="margin:0" role="search">
                    <Icon name="search"/>
                    <input class="input" type="search" aria-label="Search people" placeholder="Search people" prop:value=move || q.get() on:input=move |e| q.set(event_target_value(&e))/>
                </div>
                <Show when=move || s.directory.with(|d| d.groups.is_empty())>
                    <div class="empty"><h2>"You're not in any groups yet"</h2><p>"Ask your admin to add you to a group so you can find colleagues."</p></div>
                </Show>
                <For each=sections key=|(g, users)| (g.id, users.iter().map(|u| u.id).collect::<Vec<_>>()) let:sec>
                    {
                        let (g, users) = sec;
                        let heading = format!("group-{}", g.id);
                        let labelled = heading.clone();
                        let count = users.len();
                        view! {
                            <section aria-labelledby=labelled>
                                <div class="group-head"><h2 id=heading>{g.name.clone()}</h2><span class="hint">{format!("{count}")}</span></div>
                                <ul class="person-list">
                                    {users.into_iter().map(|u| {
                                        let uid = u.id;
                                        let presence = Signal::derive(move || store().presence_of(uid));
                                        view! {
                                            <li class="person">
                                                <Avatar name=Signal::stored(u.display_name.clone()) seed=Signal::stored(u.id.to_string()) presence=presence/>
                                                <span class="grow">
                                                    <strong>{u.display_name.clone()}</strong>
                                                    <span class="hint" style="display:block">{move || format!("@{}, {}", u.username, presence_label(presence.get()).to_lowercase())}</span>
                                                </span>
                                                <button class="btn quiet small" on:click=move |_| message(uid)><Icon name="chat"/>"Message"</button>
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                                {(count == 0).then(|| view! { <p class="hint">"No matches in this group."</p> })}
                            </section>
                        }
                    }
                </For>
            </div>
        </main>
    }
}
