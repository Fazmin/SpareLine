//! First run (join with an invite / setup code) and sign-in.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::client_ipc::*;
use sl_ui::{Icon, Spinner, ipc};

use crate::state::store;

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Choose,
    Invite,
    Setup,
}

/// Onboarding headline. Kept tool-agnostic: SpareLine covers any cloud chat, email or
/// meeting service people normally rely on.
const HERO_HEADLINE: &str = "Keep talking when the cloud goes down.";

fn hero() -> impl IntoView {
    view! {
        <section class="onboard-hero" aria-labelledby="hero-title">
            <div class="hero-lines" aria-hidden="true"><i></i><i></i></div>
            <h1 id="hero-title">{HERO_HEADLINE}</h1>
            <p>
                "SpareLine is your organisation's backup chat. It runs on a computer in your office, "
                "so it keeps working when cloud services don't. Messages are encrypted on your device; "
                "nobody else can read them, including your admin."
            </p>
        </section>
    }
}

/// Discovery shared by the invite and setup-code forms.
#[derive(Clone, Copy)]
struct Finder {
    found: RwSignal<Vec<DiscoveredServer>>,
    searching: RwSignal<bool>,
    choice: RwSignal<String>,
    manual: RwSignal<String>,
}

impl Finder {
    fn new() -> Self {
        let f = Self {
            found: RwSignal::new(vec![]),
            searching: RwSignal::new(false),
            choice: RwSignal::new(String::new()),
            manual: RwSignal::new(String::new()),
        };
        f.search();
        f
    }
    fn search(&self) {
        let f = *self;
        f.searching.set(true);
        spawn_local(async move {
            let list = ipc::invoke0::<Vec<DiscoveredServer>>("discover")
                .await
                .unwrap_or_default();
            if f.choice.get_untracked().is_empty()
                && let Some(first) = list.first().and_then(|s| s.addresses.first())
            {
                f.choice.set(first.clone());
            }
            f.found.set(list);
            f.searching.set(false);
        });
    }
    fn address(&self) -> String {
        let c = self.choice.get_untracked();
        if c == "manual" {
            self.manual.get_untracked().trim().to_string()
        } else {
            c
        }
    }
}

#[component]
fn ServerPicker(finder: Finder, #[prop(into)] hints: Signal<Vec<String>>) -> impl IntoView {
    let f = finder;
    let group = sl_ui::uid("server");
    let opt = move |value: String, title: String, detail: String| {
        let v = value.clone();
        let g = group.clone();
        view! {
            <label class="server-opt">
                <input type="radio" name=g prop:checked=move || f.choice.get() == v on:change=move |_| f.choice.set(value.clone())/>
                <span class="grow">
                    <strong>{title}</strong>
                    <span class="hint mono" style="display:block">{detail}</span>
                </span>
            </label>
        }
    };
    let opt2 = opt.clone();
    let opt3 = opt.clone();
    view! {
        <fieldset class="stack" style="border:0;padding:0;margin:0;gap:var(--s2)">
            <legend class="label" style="margin-bottom:var(--s2)">"Server"</legend>
            {move || f.found.get().into_iter().filter_map(|s| {
                let addr = s.addresses.first()?.clone();
                Some(opt(addr.clone(), s.name.clone(), s.addresses.join(", ")))
            }).collect_view()}
            {move || {
                let found: Vec<String> = f.found.get().iter().flat_map(|s| s.addresses.clone()).collect();
                hints.get().into_iter().filter(|h| !found.contains(h)).map(|h| opt2(h.clone(), "From your invite".into(), h)).collect_view()
            }}
            {opt3("manual".into(), "Enter the address myself".into(), "For example 192.168.1.10".into())}
            <Show when=move || f.choice.get() == "manual">
                <input
                    class="input mono"
                    aria-label="Server address"
                    placeholder="192.168.1.10 or office-pc.local"
                    prop:value=move || f.manual.get()
                    on:input=move |e| f.manual.set(event_target_value(&e))
                />
            </Show>
            <div class="row small muted" aria-live="polite">
                {move || if f.searching.get() {
                    view! { <Spinner label="Searching"/> <span>"Looking for SpareLine on your network…"</span> }.into_any()
                } else if f.found.get().is_empty() {
                    view! {
                        <span class="grow">"No server found automatically. Use the address in your invite."</span>
                        <button type="button" class="btn ghost small" on:click=move |_| f.search()>"Search again"</button>
                    }.into_any()
                } else {
                    view! {
                        <span class="grow">"Found on your network."</span>
                        <button type="button" class="btn ghost small" on:click=move |_| f.search()>"Search again"</button>
                    }.into_any()
                }}
            </div>
        </fieldset>
    }
}

#[component]
fn ErrorNotice(error: RwSignal<Option<String>>) -> impl IntoView {
    view! {
        <Show when=move || error.get().is_some()>
            <div class="notice fail" role="alert">
                <Icon name="alert"/>
                <span>{move || error.get().unwrap_or_default()}</span>
            </div>
        </Show>
    }
}

fn valid_username(u: &str) -> bool {
    let u = u.trim();
    (3..=32).contains(&u.chars().count())
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        && u.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
}

#[component]
pub fn Onboarding() -> impl IntoView {
    let step = RwSignal::new(Step::Choose);
    view! {
        <div class="onboard">
            {hero()}
            <section class="onboard-form" aria-label="Get started">
                {move || match step.get() {
                    Step::Choose => view! {
                        <div class="stack">
                            <h2>"Get started"</h2>
                            <p class="muted">"Your admin sends you an invite. It contains a code like SPL-7K3M-…"</p>
                            <button class="choice" on:click=move |_| step.set(Step::Invite)>
                                <Icon name="invite"/>
                                <span class="grow"><strong>"I have an invite"</strong><span class="muted">"Create your SpareLine account."</span></span>
                                <Icon name="chevron-right"/>
                            </button>
                            <button class="choice" on:click=move |_| step.set(Step::Setup)>
                                <Icon name="key"/>
                                <span class="grow"><strong>"I have a setup code"</strong><span class="muted">"Use your existing account on this computer, or reset your password."</span></span>
                                <Icon name="chevron-right"/>
                            </button>
                        </div>
                    }.into_any(),
                    Step::Invite => view! { <JoinForm back=Callback::new(move |_| step.set(Step::Choose))/> }.into_any(),
                    Step::Setup => view! { <RedeemForm back=Callback::new(move |_| step.set(Step::Choose))/> }.into_any(),
                }}
            </section>
        </div>
    }
}

#[component]
fn JoinForm(back: Callback<()>) -> impl IntoView {
    let finder = Finder::new();
    let invite = RwSignal::new(String::new());
    let display = RwSignal::new(String::new());
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let parsed = Memo::new(move |_| sl_proto::code::parse_invite_text(&invite.get()));
    let hints = Signal::derive(move || parsed.get().hosts);

    // Suggest a username from the display name.
    Effect::new(move |_| {
        let d = display.get();
        if username.get_untracked().is_empty() || valid_username(&username.get_untracked()) {
            let s: String = d
                .trim()
                .to_lowercase()
                .chars()
                .map(|c| if c.is_whitespace() { '.' } else { c })
                .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
                .collect();
            if !s.is_empty() {
                username.set(s.chars().take(32).collect());
            }
        }
    });

    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        error.set(None);
        if parsed.get_untracked().code.is_none() {
            error.set(Some(
                "Paste the invite you received. It contains a code like SPL-XXXX-XXXX-XXXX-XXXX."
                    .into(),
            ));
            return;
        }
        if display.get_untracked().trim().is_empty() {
            error.set(Some("Enter your name as colleagues know you.".into()));
            return;
        }
        if !valid_username(&username.get_untracked()) {
            error.set(Some(
                "Usernames use 3–32 letters, numbers, dots, dashes or underscores.".into(),
            ));
            return;
        }
        if password.get_untracked().chars().count() < 10 {
            error.set(Some("Choose a password of at least 10 characters.".into()));
            return;
        }
        if password.get_untracked() != confirm.get_untracked() {
            error.set(Some("The two passwords don't match.".into()));
            return;
        }
        busy.set(true);
        let input = JoinInput {
            invite_text: invite.get_untracked(),
            address: finder.address(),
            username: username.get_untracked().trim().to_string(),
            display_name: display.get_untracked().trim().to_string(),
            password: password.get_untracked(),
        };
        spawn_local(async move {
            #[derive(Serialize)]
            struct A {
                input: JoinInput,
            }
            match ipc::invoke::<AppState>("join", A { input }).await {
                Ok(s) => store().app.set(s),
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };

    view! {
        <form class="stack" on:submit=submit novalidate>
            <div class="row">
                <button type="button" class="icon-btn" aria-label="Back" on:click=move |_| back.run(())><Icon name="chevron-left"/></button>
                <h2 class="grow">"Join SpareLine"</h2>
            </div>
            <div class="field">
                <label for="invite">"Invite"</label>
                <textarea
                    id="invite"
                    class="textarea mono"
                    rows="3"
                    placeholder="Paste the whole invite message, or just the code"
                    aria-describedby="invite-hint"
                    prop:value=move || invite.get()
                    on:input=move |e| invite.set(event_target_value(&e))
                ></textarea>
                <span id="invite-hint" class="hint" aria-live="polite">
                    {move || match parsed.get().code {
                        Some(c) => format!("Code found: {}", sl_proto::code::display(&c)),
                        None => "The code looks like SPL-XXXX-XXXX-XXXX-XXXX.".into(),
                    }}
                </span>
            </div>
            <ServerPicker finder=finder hints=hints/>
            <div class="field">
                <label for="display">"Your name"</label>
                <input id="display" class="input" autocomplete="name" placeholder="Alex Morgan" prop:value=move || display.get() on:input=move |e| display.set(event_target_value(&e))/>
            </div>
            <div class="field">
                <label for="username">"Username"</label>
                <input id="username" class="input" autocomplete="username" aria-describedby="username-hint" prop:value=move || username.get() on:input=move |e| username.set(event_target_value(&e))/>
                <span id="username-hint" class="hint">"You'll use this to sign in. Letters, numbers and dots."</span>
            </div>
            <div class="field">
                <label for="pw">"Password"</label>
                <input id="pw" class="input" type="password" autocomplete="new-password" aria-describedby="pw-hint" prop:value=move || password.get() on:input=move |e| password.set(event_target_value(&e))/>
                <span id="pw-hint" class="hint">"At least 10 characters. A short sentence works well."</span>
            </div>
            <div class="field">
                <label for="pw2">"Confirm password"</label>
                <input id="pw2" class="input" type="password" autocomplete="new-password" prop:value=move || confirm.get() on:input=move |e| confirm.set(event_target_value(&e))/>
            </div>
            <ErrorNotice error=error/>
            <div class="form-actions">
                <button type="submit" class="btn wide" disabled=move || busy.get()>
                    {move || if busy.get() { view! { <Spinner/> "Checking the server's identity…" }.into_any() } else { "Join SpareLine".into_any() }}
                </button>
            </div>
            <p class="hint">"SpareLine confirms the server really issued your invite before sending any details."</p>
        </form>
    }
}

#[component]
fn RedeemForm(back: Callback<()>) -> impl IntoView {
    let finder = Finder::new();
    let code = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let parsed = Memo::new(move |_| sl_proto::code::parse_invite_text(&code.get()));
    let hints = Signal::derive(move || parsed.get().hosts);
    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        error.set(None);
        if parsed.get_untracked().code.is_none() {
            error.set(Some("Enter the setup code from your admin.".into()));
            return;
        }
        if password.get_untracked().chars().count() < 10 {
            error.set(Some("Choose a password of at least 10 characters.".into()));
            return;
        }
        if password.get_untracked() != confirm.get_untracked() {
            error.set(Some("The two passwords don't match.".into()));
            return;
        }
        busy.set(true);
        let input = RedeemInput {
            invite_text: code.get_untracked(),
            address: finder.address(),
            new_password: password.get_untracked(),
        };
        spawn_local(async move {
            #[derive(Serialize)]
            struct A {
                input: RedeemInput,
            }
            match ipc::invoke::<AppState>("redeem", A { input }).await {
                Ok(s) => store().app.set(s),
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    view! {
        <form class="stack" on:submit=submit novalidate>
            <div class="row">
                <button type="button" class="icon-btn" aria-label="Back" on:click=move |_| back.run(())><Icon name="chevron-left"/></button>
                <h2 class="grow">"Use a setup code"</h2>
            </div>
            <p class="muted">"Ask your admin for a setup code if you're on a new computer or forgot your password. You'll choose a new password, and other devices will be signed out."</p>
            <div class="field">
                <label for="code">"Setup code"</label>
                <input id="code" class="input code" placeholder="SPL-XXXX-XXXX-XXXX-XXXX" prop:value=move || code.get() on:input=move |e| code.set(event_target_value(&e))/>
            </div>
            <ServerPicker finder=finder hints=hints/>
            <div class="field">
                <label for="npw">"New password"</label>
                <input id="npw" class="input" type="password" autocomplete="new-password" prop:value=move || password.get() on:input=move |e| password.set(event_target_value(&e))/>
            </div>
            <div class="field">
                <label for="npw2">"Confirm new password"</label>
                <input id="npw2" class="input" type="password" autocomplete="new-password" prop:value=move || confirm.get() on:input=move |e| confirm.set(event_target_value(&e))/>
            </div>
            <ErrorNotice error=error/>
            <button type="submit" class="btn wide" disabled=move || busy.get()>
                {move || if busy.get() { view! { <Spinner/> "Checking the server's identity…" }.into_any() } else { "Continue".into_any() }}
            </button>
        </form>
    }
}

#[component]
pub fn SignIn() -> impl IntoView {
    let s = store();
    let username = RwSignal::new(
        s.app
            .get_untracked()
            .me
            .map(|m| m.username)
            .unwrap_or_default(),
    );
    let password = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let setup = RwSignal::new(false);
    let forget_open = RwSignal::new(false);
    let server = move || s.app.get().server;
    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        error.set(None);
        busy.set(true);
        let input = LoginInput {
            username: username.get_untracked(),
            password: password.get_untracked(),
        };
        spawn_local(async move {
            #[derive(Serialize)]
            struct A {
                input: LoginInput,
            }
            match ipc::invoke::<AppState>("login", A { input }).await {
                Ok(st) => store().app.set(st),
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    let forget = move |_| {
        spawn_local(async move {
            #[derive(Serialize)]
            struct A {
                forget: bool,
            }
            if let Ok(st) = ipc::invoke::<AppState>("logout", A { forget: true }).await {
                store().app.set(st);
            }
        });
    };
    view! {
        <div class="onboard">
            {hero()}
            <section class="onboard-form" aria-label="Sign in">
                <Show
                    when=move || !setup.get()
                    fallback=move || view! { <RedeemForm back=Callback::new(move |_| setup.set(false))/> }
                >
                    <form class="stack" on:submit=submit novalidate>
                        <h2>{move || format!("Sign in to {}", server().map(|s| s.name).unwrap_or_else(|| "SpareLine".into()))}</h2>
                        <div class="field">
                            <label for="si-user">"Username"</label>
                            <input id="si-user" class="input" autocomplete="username" prop:value=move || username.get() on:input=move |e| username.set(event_target_value(&e))/>
                        </div>
                        <div class="field">
                            <label for="si-pw">"Password"</label>
                            <input id="si-pw" class="input" type="password" autocomplete="current-password" autofocus prop:value=move || password.get() on:input=move |e| password.set(event_target_value(&e))/>
                        </div>
                        <ErrorNotice error=error/>
                        <button type="submit" class="btn wide" disabled=move || busy.get()>
                            {move || if busy.get() { view! { <Spinner/> "Signing in…" }.into_any() } else { "Sign in".into_any() }}
                        </button>
                        <button type="button" class="btn ghost" on:click=move |_| setup.set(true)>"Forgot your password? Use a setup code"</button>
                        <details class="hint">
                            <summary>"Server details"</summary>
                            <dl class="kv" style="margin-top:var(--s2)">
                                <dt>"Address"</dt>
                                <dd class="mono">{move || server().map(|s| s.last_address).unwrap_or_default()}</dd>
                                <dt>"Fingerprint"</dt>
                                <dd class="fingerprint">{move || server().map(|s| s.fingerprint).unwrap_or_default()}</dd>
                            </dl>
                            <button type="button" class="btn quiet small" style="margin-top:var(--s3)" on:click=move |_| forget_open.set(true)>"Use a different server"</button>
                        </details>
                    </form>
                </Show>
                <sl_ui::Modal open=forget_open title="Use a different server?">
                    <div class="modal-body"><p>"This removes the saved server and your chat history from this computer. Your account on the server is not affected."</p></div>
                    <div class="modal-foot">
                        <button class="btn quiet" on:click=move |_| forget_open.set(false)>"Cancel"</button>
                        <button class="btn danger" on:click=forget>"Remove and start over"</button>
                    </div>
                </sl_ui::Modal>
            </section>
        </div>
    }
}
