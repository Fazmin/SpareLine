//! Appearance and accessibility preferences, stored per device in localStorage and
//! applied as attributes on `<html>` (see `styles/sl.css`).
//!
//! `index.html` applies the same stored values with a tiny inline script before the
//! app loads, so the chosen theme never flashes.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;

pub const STORAGE_KEY: &str = "spareline.prefs";
/// Text sizes offered (87.5% to 200%, WCAG 1.4.4).
pub const SCALES: [f64; 8] = [0.875, 1.0, 1.125, 1.25, 1.375, 1.5, 1.75, 2.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: ThemePref,
    pub scale: f64,
    pub high_contrast: bool,
    pub reduce_motion: bool,
    pub compact: bool,
    /// Atkinson Hyperlegible instead of the platform UI font.
    pub legible_font: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: ThemePref::System,
            scale: 1.0,
            high_contrast: false,
            reduce_motion: false,
            compact: false,
            legible_font: false,
        }
    }
}

impl Prefs {
    pub fn bigger(&mut self) {
        if let Some(next) = SCALES.iter().find(|s| **s > self.scale + 0.001) {
            self.scale = *next;
        }
    }
    pub fn smaller(&mut self) {
        if let Some(prev) = SCALES.iter().rev().find(|s| **s < self.scale - 0.001) {
            self.scale = *prev;
        }
    }
    pub fn percent(&self) -> u32 {
        (self.scale * 100.0).round() as u32
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

pub fn load() -> Prefs {
    storage()
        .and_then(|s| s.get_item(STORAGE_KEY).ok().flatten())
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

pub fn save(p: &Prefs) {
    if let (Some(s), Ok(v)) = (storage(), serde_json::to_string(p)) {
        let _ = s.set_item(STORAGE_KEY, &v);
    }
}

pub fn system_dark() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
        .is_some_and(|m| m.matches())
}

pub fn apply(p: &Prefs) {
    let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    else {
        return;
    };
    let dark = match p.theme {
        ThemePref::Dark => true,
        ThemePref::Light => false,
        ThemePref::System => system_dark(),
    };
    let _ = el.set_attribute("data-theme", if dark { "dark" } else { "light" });
    let toggle = |name: &str, on: bool, value: &str| {
        if on {
            let _ = el.set_attribute(name, value);
        } else {
            let _ = el.remove_attribute(name);
        }
    };
    toggle("data-contrast", p.high_contrast, "high");
    toggle("data-motion", p.reduce_motion, "reduce");
    toggle("data-density", p.compact, "compact");
    toggle("data-font", p.legible_font, "legible");
    if let Some(h) = el.dyn_ref::<web_sys::HtmlElement>() {
        let _ = h.style().set_property("--scale", &format!("{}", p.scale));
    }
}

#[derive(Clone, Copy)]
pub struct PrefsCtx(pub RwSignal<Prefs>);

/// Provide preferences to the app: apply + persist on change, follow the system theme,
/// and install text-size shortcuts (Ctrl/Cmd + `=`, `-`, `0`).
pub fn provide_prefs() -> PrefsCtx {
    let sig = RwSignal::new(load());
    Effect::new(move |_| {
        let p = sig.get();
        apply(&p);
        save(&p);
    });
    // Follow OS theme changes while on "System".
    if let Some(mql) =
        web_sys::window().and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
    {
        let cb = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
            let p = sig.get_untracked();
            if p.theme == ThemePref::System {
                apply(&p);
            }
        });
        let _ = mql.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    let handle = window_event_listener(leptos::ev::keydown, move |e| {
        if !(e.ctrl_key() || e.meta_key()) {
            return;
        }
        match e.key().as_str() {
            "=" | "+" => {
                e.prevent_default();
                sig.update(|p| p.bigger());
            }
            "-" | "_" => {
                e.prevent_default();
                sig.update(|p| p.smaller());
            }
            "0" => {
                e.prevent_default();
                sig.update(|p| p.scale = 1.0);
            }
            _ => {}
        }
    });
    // Lives for the whole app.
    std::mem::forget(handle);
    let ctx = PrefsCtx(sig);
    provide_context(ctx);
    ctx
}

/// The inline script used by both `index.html` files (kept here for reference/tests).
pub const PREPAINT_SCRIPT: &str = r#"(function(){try{var p=JSON.parse(localStorage.getItem('spareline.prefs')||'{}');var d=p.theme==='dark'||(p.theme!=='light'&&matchMedia('(prefers-color-scheme: dark)').matches);var e=document.documentElement;e.setAttribute('data-theme',d?'dark':'light');if(p.high_contrast)e.setAttribute('data-contrast','high');if(p.reduce_motion)e.setAttribute('data-motion','reduce');if(p.compact)e.setAttribute('data-density','compact');if(p.legible_font)e.setAttribute('data-font','legible');if(p.scale)e.style.setProperty('--scale',p.scale);}catch(_){}})();"#;
