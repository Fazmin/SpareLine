//! Small shared widgets.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use leptos::prelude::*;
use sl_proto::Presence;

use crate::format;
use crate::icons::Icon;
use crate::prefs::{PrefsCtx, ThemePref};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A unique DOM id with a readable prefix.
pub fn uid(prefix: &str) -> String {
    format!("{prefix}-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

/// The label for the platform's shortcut modifier: "⌘" on macOS, "Ctrl " elsewhere
/// (with a space, so "Ctrl K" reads as two keys).
pub fn mod_key() -> &'static str {
    let mac = web_sys::window()
        .and_then(|w| w.navigator().user_agent().ok())
        .is_some_and(|ua| ua.contains("Mac"));
    if mac { "⌘" } else { "Ctrl " }
}

// -------------------------------------------------------------------------------------
// The line
// -------------------------------------------------------------------------------------

/// The connection line across the top of the window. `state` is one of
/// `connected`, `connecting`, `reconnecting`, `offline`, `idle`.
#[component]
pub fn SignalLine(
    #[prop(into)] state: Signal<String>,
    #[prop(into)] label: Signal<String>,
) -> impl IntoView {
    view! {
        <div class="signal-line" data-state=move || state.get() aria-hidden="true"></div>
        <div class="sr-only" role="status" aria-live="polite">{move || label.get()}</div>
    }
}

/// The SpareLine mark: the main line and the spare.
#[component]
pub fn Mark() -> impl IntoView {
    view! { <span class="mark" aria-hidden="true"><i></i><i></i></span> }
}

// -------------------------------------------------------------------------------------
// People
// -------------------------------------------------------------------------------------

pub fn presence_str(p: Presence) -> &'static str {
    match p {
        Presence::Online => "online",
        Presence::Away => "away",
        Presence::Offline => "offline",
    }
}

pub fn presence_label(p: Presence) -> &'static str {
    match p {
        Presence::Online => "Available",
        Presence::Away => "Away",
        Presence::Offline => "Offline",
    }
}

#[component]
pub fn PresenceDot(#[prop(into)] presence: Signal<Presence>) -> impl IntoView {
    view! {
        <span
            class="presence"
            data-p=move || presence_str(presence.get())
            role="img"
            aria-label=move || presence_label(presence.get())
            title=move || presence_label(presence.get())
        ></span>
    }
}

#[component]
pub fn Avatar(
    #[prop(into)] name: Signal<String>,
    #[prop(into)] seed: Signal<String>,
    /// Size in rem.
    #[prop(default = 2.25)]
    size: f64,
    #[prop(optional)] group: bool,
    #[prop(optional, into)] presence: Option<Signal<Presence>>,
) -> impl IntoView {
    view! {
        <span
            class="avatar"
            class:group=group
            data-hue=move || format::hue(&seed.get()).to_string()
            style=format!("--av: {size}rem")
            aria-hidden="true"
        >
            {move || format::initials(&name.get())}
            {presence.map(|p| view! { <PresenceDot presence=p/> })}
        </span>
    }
}

// -------------------------------------------------------------------------------------
// Toasts
// -------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub level: &'static str,
    pub text: String,
}

#[derive(Clone, Copy)]
pub struct Toasts(pub RwSignal<Vec<Toast>>);

impl Toasts {
    pub fn show(&self, level: &'static str, text: impl Into<String>) {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let sig = self.0;
        sig.update(|v| {
            v.push(Toast {
                id,
                level,
                text: text.into(),
            });
            if v.len() > 4 {
                v.remove(0);
            }
        });
        set_timeout(
            move || sig.update(|v| v.retain(|t| t.id != id)),
            Duration::from_secs(6),
        );
    }
    pub fn info(&self, text: impl Into<String>) {
        self.show("info", text);
    }
    pub fn warn(&self, text: impl Into<String>) {
        self.show("warning", text);
    }
    pub fn error(&self, text: impl Into<String>) {
        self.show("critical", text);
    }
}

pub fn provide_toasts() -> Toasts {
    let t = Toasts(RwSignal::new(Vec::new()));
    provide_context(t);
    t
}

pub fn toasts() -> Toasts {
    expect_context::<Toasts>()
}

#[component]
pub fn ToastRegion() -> impl IntoView {
    let t = toasts();
    view! {
        <div class="toasts" role="status" aria-live="polite" aria-atomic="false">
            <For each=move || t.0.get() key=|x| x.id let:x>
                <div class="toast" data-level=x.level>
                    <span class="grow">{x.text.clone()}</span>
                    <button
                        class="icon-btn"
                        style="width:1.8rem;height:1.8rem;color:inherit"
                        aria-label="Dismiss"
                        on:click=move |_| t.0.update(|v| v.retain(|y| y.id != x.id))
                    >
                        <Icon name="close"/>
                    </button>
                </div>
            </For>
        </div>
    }
}

// -------------------------------------------------------------------------------------
// Modal (native <dialog>: focus trap, Esc, inert background)
// -------------------------------------------------------------------------------------

#[component]
pub fn Modal(
    open: RwSignal<bool>,
    #[prop(into)] title: Signal<String>,
    #[prop(optional)] wide: bool,
    /// Command-menu style: pinned near the top, title read by screen readers only,
    /// closed with Esc or a click outside.
    #[prop(optional)]
    palette: bool,
    children: ChildrenFn,
) -> impl IntoView {
    let dlg: NodeRef<leptos::html::Dialog> = NodeRef::new();
    let title_id = uid("dlg-title");
    Effect::new(move |_| {
        let want = open.get();
        if let Some(d) = dlg.get() {
            if want && !d.open() {
                let _ = d.show_modal();
            } else if !want && d.open() {
                d.close();
            }
        }
    });
    let children = StoredValue::new(children);
    let labelled_by = title_id.clone();
    view! {
        <dialog
            class="modal"
            class:wide=wide
            class:palette=palette
            node_ref=dlg
            aria-labelledby=labelled_by
            on:close=move |_| open.set(false)
            on:click=move |e| {
                // A click on the dialog element itself is a click on the backdrop.
                if palette && e.target() == e.current_target() {
                    open.set(false);
                }
            }
        >
            <div class="modal-head">
                <h2 id=title_id class="grow">{move || title.get()}</h2>
                {(!palette).then(|| view! {
                    <button class="icon-btn" aria-label="Close" on:click=move |_| open.set(false)>
                        <Icon name="close"/>
                    </button>
                })}
            </div>
            <Show when=move || open.get()>{move || children.with_value(|c| c())}</Show>
        </dialog>
    }
}

// -------------------------------------------------------------------------------------
// Command menu
// -------------------------------------------------------------------------------------

/// One entry in a [`CommandMenu`].
#[derive(Clone)]
pub struct Command {
    /// Heading the entry is listed under; consecutive entries share one heading.
    pub group: &'static str,
    pub label: String,
    /// Second line, for example a message preview.
    pub detail: String,
    /// Icon name, or empty when `avatar` is set.
    pub icon: &'static str,
    /// `(name, seed, is_group)` to show an avatar instead of an icon.
    pub avatar: Option<(String, String, bool)>,
    /// Text on the right, for example a time.
    pub trail: String,
    /// Keyboard shortcut shown on the right, one chip per key.
    pub keys: Vec<&'static str>,
    /// Plain closure rather than a `Callback`, so result lists can be rebuilt freely
    /// without leaving reactive nodes behind.
    pub run: std::sync::Arc<dyn Fn() + Send + Sync>,
}

impl Command {
    pub fn new(
        group: &'static str,
        icon: &'static str,
        label: impl Into<String>,
        run: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            group,
            label: label.into(),
            detail: String::new(),
            icon,
            avatar: None,
            trail: String::new(),
            keys: Vec::new(),
            run: std::sync::Arc::new(run),
        }
    }
    pub fn keys(mut self, keys: &[&'static str]) -> Self {
        self.keys = keys.to_vec();
        self
    }
    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = d.into();
        self
    }
    pub fn trail(mut self, t: impl Into<String>) -> Self {
        self.trail = t.into();
        self
    }
    pub fn avatar(mut self, name: impl Into<String>, seed: impl Into<String>, group: bool) -> Self {
        self.avatar = Some((name.into(), seed.into(), group));
        self
    }
}

/// A keyboard-first command palette: type to filter, arrows to move, Enter to run.
/// The caller filters `items` from `query`, so it can mix static commands and search hits.
#[component]
pub fn CommandMenu(
    open: RwSignal<bool>,
    #[prop(into)] title: String,
    #[prop(into)] placeholder: String,
    query: RwSignal<String>,
    #[prop(into)] items: Signal<Vec<Command>>,
    /// Shown when nothing matches.
    #[prop(into)]
    empty: Signal<String>,
) -> impl IntoView {
    let active = RwSignal::new(0usize);
    let list_id = uid("cmd");
    // A new result set starts at the top.
    Effect::new(move |_| {
        items.with(|_| ());
        active.set(0);
    });
    let opt_id = {
        let list_id = list_id.clone();
        move |i: usize| format!("{list_id}-{i}")
    };
    // Keep the highlighted entry in view.
    {
        let opt_id = opt_id.clone();
        Effect::new(move |_| {
            let i = active.get();
            let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
                return;
            };
            let (Some(item), Some(list)) = (
                doc.get_element_by_id(&opt_id(i))
                    .and_then(|e| wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlElement>(e).ok()),
                doc.get_element_by_id(&opt_id(i))
                    .and_then(|e| e.parent_element()),
            ) else {
                return;
            };
            let (top, bottom) = (item.offset_top(), item.offset_top() + item.offset_height());
            if top < list.scroll_top() {
                list.set_scroll_top(top - 8);
            } else if bottom > list.scroll_top() + list.client_height() {
                list.set_scroll_top(bottom - list.client_height() + 8);
            }
        });
    }
    let run = move |i: usize| {
        let Some(cmd) = items.with_untracked(|v| v.get(i).cloned()) else {
            return;
        };
        open.set(false);
        query.set(String::new());
        (cmd.run)();
    };
    let on_keydown = move |e: leptos::ev::KeyboardEvent| {
        let n = items.with_untracked(Vec::len);
        match e.key().as_str() {
            "ArrowDown" if n > 0 => {
                e.prevent_default();
                active.update(|a| *a = (*a + 1) % n);
            }
            "ArrowUp" if n > 0 => {
                e.prevent_default();
                active.update(|a| *a = (*a + n - 1) % n);
            }
            "Enter" => {
                e.prevent_default();
                run(active.get_untracked());
            }
            _ => {}
        }
    };
    let active_desc = {
        let opt_id = opt_id.clone();
        Signal::derive(move || (!items.with(Vec::is_empty)).then(|| opt_id(active.get())))
    };
    let list_for_input = list_id.clone();
    let title_for_list = title.clone();
    view! {
        <Modal open=open title=title palette=true>
            <div class="cmd">
                <div class="cmd-search">
                    <Icon name="search"/>
                    <input
                        class="cmd-input"
                        type="text"
                        role="combobox"
                        aria-expanded="true"
                        aria-autocomplete="list"
                        aria-controls=list_for_input.clone()
                        aria-activedescendant=move || active_desc.get()
                        aria-label=placeholder.clone()
                        placeholder=placeholder.clone()
                        autofocus
                        prop:value=move || query.get()
                        on:input=move |e| query.set(event_target_value(&e))
                        on:keydown=on_keydown
                    />
                </div>
                <ul class="cmd-list" id=list_id.clone() role="listbox" aria-label=title_for_list.clone()>
                    {
                        let opt_id = opt_id.clone();
                        move || {
                            let v = items.get();
                            if v.is_empty() {
                                return view! { <li class="cmd-empty" role="presentation">{move || empty.get()}</li> }.into_any();
                            }
                            let mut last: Option<&'static str> = None;
                            v.into_iter().enumerate().map(|(i, c)| {
                                let head = (last != Some(c.group)).then(|| view! { <li class="cmd-group" role="presentation">{c.group}</li> });
                                last = Some(c.group);
                                let lead = match c.avatar.clone() {
                                    Some((n, s, g)) => view! { <Avatar name=Signal::stored(n) seed=Signal::stored(s) size=1.5 group=g/> }.into_any(),
                                    None => view! { <Icon name=c.icon/> }.into_any(),
                                };
                                let has_detail = !c.detail.is_empty();
                                view! {
                                    {head}
                                    <li
                                        class="cmd-item"
                                        role="option"
                                        id=opt_id(i)
                                        aria-selected=move || (active.get() == i).to_string()
                                        on:mousemove=move |_| if active.get_untracked() != i { active.set(i) }
                                        on:click=move |_| run(i)
                                    >
                                        {lead}
                                        <span class="grow">
                                            <span class="cmd-label truncate">{c.label.clone()}</span>
                                            {has_detail.then(|| view! { <span class="cmd-detail">{c.detail.clone()}</span> })}
                                        </span>
                                        {(!c.trail.is_empty()).then(|| view! { <span class="cmd-trail">{c.trail.clone()}</span> })}
                                        {(!c.keys.is_empty()).then(|| view! {
                                            <span class="cmd-trail" aria-hidden="true">
                                                {c.keys.iter().map(|k| view! { <span class="kbd">{*k}</span> }).collect_view()}
                                            </span>
                                        })}
                                    </li>
                                }
                            }).collect_view().into_any()
                        }
                    }
                </ul>
                <div class="cmd-foot" aria-hidden="true">
                    <span><span class="kbd">"↑"</span><span class="kbd">"↓"</span>" to move"</span>
                    <span><span class="kbd">"↵"</span>" to choose"</span>
                    <span class="grow"></span>
                    <span><span class="kbd">"Esc"</span>" to close"</span>
                </div>
            </div>
        </Modal>
    }
}

// -------------------------------------------------------------------------------------
// Form helpers
// -------------------------------------------------------------------------------------

#[component]
pub fn Switch(
    #[prop(into)] label: String,
    #[prop(optional, into)] hint: String,
    checked: Signal<bool>,
    on_change: Callback<bool>,
) -> impl IntoView {
    let id = uid("sw");
    let hint_id = uid("sw-hint");
    let has_hint = !hint.is_empty();
    let for_id = id.clone();
    view! {
        <label class="switch" for=for_id>
            <span class="stack" style="gap:0">
                <span style="font-weight:500">{label}</span>
                {has_hint.then(|| view! { <span class="hint" id=hint_id.clone()>{hint.clone()}</span> })}
            </span>
            <input
                id=id
                type="checkbox"
                role="switch"
                aria-describedby=has_hint.then(|| hint_id.clone())
                prop:checked=move || checked.get()
                on:change=move |ev| on_change.run(event_target_checked(&ev))
            />
        </label>
    }
}

// -------------------------------------------------------------------------------------
// Appearance & accessibility
// -------------------------------------------------------------------------------------

/// Theme, text size, contrast, motion and density controls.
#[component]
pub fn AppearanceControls() -> impl IntoView {
    let PrefsCtx(prefs) = expect_context::<PrefsCtx>();
    let group = uid("theme");
    let theme_opt = move |value: ThemePref, label: &'static str| {
        let group = group.clone();
        view! {
            <label>
                <input
                    type="radio"
                    name=group
                    prop:checked=move || prefs.get().theme == value
                    on:change=move |_| prefs.update(|p| p.theme = value)
                />
                {label}
            </label>
        }
    };
    // Comparisons are computed here: a bare `>` inside a view! attribute ends the tag.
    let at_min = move || prefs.get().scale <= crate::prefs::SCALES[0];
    let at_max = move || prefs.get().scale >= crate::prefs::SCALES[crate::prefs::SCALES.len() - 1];
    view! {
        <div class="stack" style="gap:var(--s4)">
            <fieldset class="stack" style="border:0;padding:0;margin:0;gap:var(--s2)">
                <legend class="label" style="margin-bottom:var(--s2)">"Theme"</legend>
                <div class="seg" role="radiogroup" aria-label="Theme">
                    {theme_opt(ThemePref::System, "System")}
                    {theme_opt(ThemePref::Light, "Light")}
                    {theme_opt(ThemePref::Dark, "Dark")}
                </div>
            </fieldset>
            <div class="stack" style="gap:var(--s2)">
                <span class="label" id="text-size-label">"Text size"</span>
                <div class="row" role="group" aria-labelledby="text-size-label">
                    <button
                        class="btn quiet small"
                        aria-label="Smaller text"
                        disabled=at_min
                        on:click=move |_| prefs.update(|p| p.smaller())
                    >
                        <span style="font-size:0.85em">"A"</span>
                        "−"
                    </button>
                    <output class="grow" style="text-align:center;font-weight:600" aria-live="polite">
                        {move || format!("{}%", prefs.get().percent())}
                    </output>
                    <button
                        class="btn quiet small"
                        aria-label="Larger text"
                        disabled=at_max
                        on:click=move |_| prefs.update(|p| p.bigger())
                    >
                        <span style="font-size:1.15em">"A"</span>
                        "+"
                    </button>
                </div>
                <span class="hint">
                    "Shortcut: " <span class="kbd">"Ctrl"</span> " + " <span class="kbd">"+"</span> " / "
                    <span class="kbd">"−"</span> ", reset with " <span class="kbd">"0"</span>
                </span>
            </div>
            <Switch
                label="Easier-to-read font"
                hint="Atkinson Hyperlegible: letters like I, l and 1 are easy to tell apart."
                checked=Signal::derive(move || prefs.get().legible_font)
                on_change=Callback::new(move |v| prefs.update(|p| p.legible_font = v))
            />
            <Switch
                label="High contrast"
                hint="Black and white with stronger outlines."
                checked=Signal::derive(move || prefs.get().high_contrast)
                on_change=Callback::new(move |v| prefs.update(|p| p.high_contrast = v))
            />
            <Switch
                label="Reduce motion"
                hint="Stops animations, including the moving connection line."
                checked=Signal::derive(move || prefs.get().reduce_motion)
                on_change=Callback::new(move |v| prefs.update(|p| p.reduce_motion = v))
            />
            <Switch
                label="Compact layout"
                hint="Fits more on screen."
                checked=Signal::derive(move || prefs.get().compact)
                on_change=Callback::new(move |v| prefs.update(|p| p.compact = v))
            />
        </div>
    }
}

/// The "Aa" button that opens appearance controls in a popover.
#[component]
pub fn AaButton(#[prop(optional)] align_left: bool) -> impl IntoView {
    let open = RwSignal::new(false);
    let pop_id = uid("aa");
    let close = move || open.set(false);
    view! {
        <div style="position:relative">
            <button
                class="icon-btn"
                aria-label="Appearance and accessibility"
                title="Appearance and accessibility"
                aria-expanded=move || open.get().to_string()
                aria-controls=pop_id.clone()
                on:click=move |_| open.update(|o| *o = !*o)
                style="font-weight:600;letter-spacing:-0.02em;font-size:var(--t-sm)"
            >
                "Aa"
            </button>
            <Show when=move || open.get()>
                <div style="position:fixed;inset:0;z-index:39" on:click=move |_| close()></div>
                <div
                    class="popover"
                    id=pop_id.clone()
                    role="dialog"
                    aria-label="Appearance and accessibility"
                    style=if align_left { "left:0;bottom:calc(100% + 6px)" } else { "right:0;top:calc(100% + 6px)" }
                    on:keydown=move |e| {
                        if e.key() == "Escape" {
                            close();
                        }
                    }
                >
                    <div class="row" style="margin-bottom:var(--s3)">
                        <h2 class="grow">"Appearance"</h2>
                        <button class="btn ghost small" on:click=move |_| close()>"Done"</button>
                    </div>
                    <AppearanceControls/>
                </div>
            </Show>
        </div>
    }
}

/// A plain spinner with an accessible label.
#[component]
pub fn Spinner(#[prop(optional, into)] label: String) -> impl IntoView {
    let label = if label.is_empty() {
        "Working…".to_string()
    } else {
        label
    };
    view! { <span class="spinner" role="status" aria-label=label></span> }
}
