//! Typed calls into the Tauri backend (`window.__TAURI__`, enabled with `withGlobalTauri`).

use serde::Serialize;
use serde::de::DeserializeOwned;
use sl_proto::client_ipc::UiError;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"], js_name = invoke)]
    async fn tauri_invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "event"], js_name = listen)]
    async fn tauri_listen(event: &str, handler: &js_sys::Function) -> Result<JsValue, JsValue>;
}

type MockFn = Box<dyn Fn(&str, serde_json::Value) -> Result<serde_json::Value, UiError>>;

thread_local! {
    static MOCK: std::cell::RefCell<Option<MockFn>> = const { std::cell::RefCell::new(None) };
}

/// Browser preview: answer commands from canned data instead of the Tauri backend.
pub fn set_mock(
    f: impl Fn(&str, serde_json::Value) -> Result<serde_json::Value, UiError> + 'static,
) {
    MOCK.with(|m| *m.borrow_mut() = Some(Box::new(f)));
}

/// True if the page URL asks for the browser preview (`?demo`).
pub fn demo_requested() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let q = search.trim_start_matches('?');
    q.split('&').find(|p| p.starts_with("demo")).map(|p| {
        p.split_once('=')
            .map(|(_, v)| v.to_string())
            .unwrap_or_default()
    })
}

/// True when running inside the Tauri webview.
pub fn in_tauri() -> bool {
    web_sys::window()
        .and_then(|w| js_sys::Reflect::get(&w, &JsValue::from_str("__TAURI__")).ok())
        .is_some_and(|v| !v.is_undefined() && !v.is_null())
}

fn err(code: &str, message: impl Into<String>) -> UiError {
    UiError {
        code: code.into(),
        message: message.into(),
    }
}

#[derive(Serialize)]
struct NoArgs {}

/// Call a command with named arguments (an object whose keys match the Rust parameter
/// names in camelCase).
pub async fn invoke<T: DeserializeOwned>(cmd: &str, args: impl Serialize) -> Result<T, UiError> {
    if !in_tauri() {
        let mocked = MOCK.with(|m| {
            m.borrow().as_ref().map(|f| {
                let a = serde_json::to_value(&args).unwrap_or(serde_json::Value::Null);
                f(cmd, a)
            })
        });
        return match mocked {
            Some(Ok(v)) => {
                serde_json::from_value(v).map_err(|e| err("decode", format!("{cmd}: {e}")))
            }
            Some(Err(e)) => Err(e),
            None => Err(err("no_tauri", "Open SpareLine from the desktop app.")),
        };
    }
    let ser = serde_wasm_bindgen::Serializer::json_compatible();
    let a = args
        .serialize(&ser)
        .map_err(|e| err("encode", e.to_string()))?;
    match tauri_invoke(cmd, a).await {
        Ok(v) => {
            if v.is_undefined() || v.is_null() {
                // Commands returning `()` resolve to null.
                return serde_wasm_bindgen::from_value(JsValue::NULL)
                    .map_err(|e| err("decode", e.to_string()));
            }
            serde_wasm_bindgen::from_value(v).map_err(|e| err("decode", e.to_string()))
        }
        Err(e) => Err(
            serde_wasm_bindgen::from_value::<UiError>(e.clone()).unwrap_or_else(|_| {
                err(
                    "error",
                    e.as_string().unwrap_or_else(|| "Unexpected error".into()),
                )
            }),
        ),
    }
}

/// Call a command without arguments.
pub async fn invoke0<T: DeserializeOwned>(cmd: &str) -> Result<T, UiError> {
    invoke(cmd, NoArgs {}).await
}

/// Subscribe to a backend event for the life of the page.
pub fn listen<T: DeserializeOwned + 'static>(event: &'static str, f: impl Fn(T) + 'static) {
    if !in_tauri() {
        return;
    }
    let cb = Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
        let payload =
            js_sys::Reflect::get(&ev, &JsValue::from_str("payload")).unwrap_or(JsValue::NULL);
        match serde_wasm_bindgen::from_value::<T>(payload) {
            Ok(t) => f(t),
            Err(e) => web_sys::console::warn_1(&JsValue::from_str(&format!(
                "bad event payload on {event}: {e}"
            ))),
        }
    });
    let func: js_sys::Function = cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
    cb.forget();
    wasm_bindgen_futures::spawn_local(async move {
        let _ = tauri_listen(event, &func).await;
    });
}

/// Copy text to the clipboard.
pub async fn copy_text(text: &str) -> bool {
    let Some(w) = web_sys::window() else {
        return false;
    };
    let p = w.navigator().clipboard().write_text(text);
    wasm_bindgen_futures::JsFuture::from(p).await.is_ok()
}
