//! SpareLine desktop client: Tauri shell around `sl-client-core`.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::sync::Arc;

use serde::Serialize;
use sl_client_core::{Client, ClientError, ClientOptions, FileStore, KeyringStore, SecretStore};
use sl_proto::client_ipc::*;
use sl_proto::{ConversationId, Directory, Presence, UserId};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_notification::NotificationExt as _;
use tauri_plugin_opener::OpenerExt as _;
use uuid::Uuid;

struct Ctx {
    client: Client,
}

type R<T> = Result<T, UiError>;

fn ui(e: ClientError) -> UiError {
    e.to_ui()
}

#[derive(Serialize)]
struct AppInfo {
    version: String,
    platform: String,
    data_dir: String,
}

#[tauri::command]
fn app_state(ctx: State<'_, Ctx>) -> AppState {
    ctx.client.state()
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        platform: std::env::consts::OS.into(),
        data_dir: sl_client_core::profile::default_dir()
            .to_string_lossy()
            .into(),
    }
}

#[tauri::command]
async fn discover(ctx: State<'_, Ctx>) -> R<Vec<DiscoveredServer>> {
    ctx.client.discover().await.map_err(ui)
}

#[tauri::command]
async fn join(ctx: State<'_, Ctx>, input: JoinInput) -> R<AppState> {
    ctx.client.join(input).await.map_err(ui)
}

#[tauri::command]
async fn redeem(ctx: State<'_, Ctx>, input: RedeemInput) -> R<AppState> {
    ctx.client.redeem(input).await.map_err(ui)
}

#[tauri::command]
async fn login(ctx: State<'_, Ctx>, input: LoginInput) -> R<AppState> {
    ctx.client.login(input).await.map_err(ui)
}

#[tauri::command]
async fn logout(ctx: State<'_, Ctx>, forget: bool) -> R<AppState> {
    ctx.client.logout(forget).await.map_err(ui)
}

#[tauri::command]
async fn reconnect_now(ctx: State<'_, Ctx>) -> R<()> {
    ctx.client.reconnect_now().await.map_err(ui)
}

#[tauri::command]
async fn conversations(ctx: State<'_, Ctx>) -> R<Vec<ConversationView>> {
    ctx.client.conversations().await.map_err(ui)
}

#[tauri::command]
async fn messages(
    ctx: State<'_, Ctx>,
    conversation: ConversationId,
    before: Option<i64>,
    limit: u32,
) -> R<Vec<MessageView>> {
    ctx.client
        .messages(conversation, before, limit)
        .await
        .map_err(ui)
}

#[tauri::command]
async fn send(ctx: State<'_, Ctx>, input: SendInput) -> R<MessageView> {
    ctx.client.send(input).await.map_err(ui)
}

#[tauri::command]
async fn edit(ctx: State<'_, Ctx>, conversation: ConversationId, id: Uuid, body: String) -> R<()> {
    ctx.client.edit(conversation, id, body).await.map_err(ui)
}

#[tauri::command]
async fn delete(ctx: State<'_, Ctx>, conversation: ConversationId, id: Uuid) -> R<()> {
    ctx.client.delete(conversation, id).await.map_err(ui)
}

#[tauri::command]
async fn react(
    ctx: State<'_, Ctx>,
    conversation: ConversationId,
    id: Uuid,
    emoji: String,
) -> R<()> {
    ctx.client.react(conversation, id, emoji).await.map_err(ui)
}

#[tauri::command]
async fn mark_read(ctx: State<'_, Ctx>, conversation: ConversationId) -> R<()> {
    ctx.client.mark_read(conversation).await.map_err(ui)
}

#[tauri::command]
async fn typing(ctx: State<'_, Ctx>, conversation: ConversationId) -> R<()> {
    ctx.client.typing(conversation).await.map_err(ui)
}

#[tauri::command]
async fn start_chat(ctx: State<'_, Ctx>, input: StartChatInput) -> R<ConversationView> {
    ctx.client.start_chat(input).await.map_err(ui)
}

#[tauri::command]
async fn add_people(
    ctx: State<'_, Ctx>,
    conversation: ConversationId,
    users: Vec<UserId>,
) -> R<()> {
    ctx.client.add_people(conversation, users).await.map_err(ui)
}

#[tauri::command]
async fn leave(ctx: State<'_, Ctx>, conversation: ConversationId) -> R<()> {
    ctx.client.leave(conversation).await.map_err(ui)
}

#[tauri::command]
async fn directory(ctx: State<'_, Ctx>) -> R<Directory> {
    ctx.client.directory().await.map_err(ui)
}

#[tauri::command]
async fn set_presence(ctx: State<'_, Ctx>, presence: Presence) -> R<()> {
    ctx.client.set_presence(presence).await.map_err(ui)
}

#[tauri::command]
async fn set_display_name(ctx: State<'_, Ctx>, name: String) -> R<()> {
    ctx.client.set_display_name(name).await.map_err(ui)
}

#[tauri::command]
async fn change_password(ctx: State<'_, Ctx>, current: String, new: String) -> R<()> {
    ctx.client.change_password(current, new).await.map_err(ui)
}

#[tauri::command]
async fn read_receipts(ctx: State<'_, Ctx>) -> R<bool> {
    ctx.client.read_receipts().await.map_err(ui)
}

#[tauri::command]
async fn set_read_receipts(ctx: State<'_, Ctx>, on: bool) -> R<()> {
    ctx.client.set_read_receipts(on).await.map_err(ui)
}

#[tauri::command]
async fn search(ctx: State<'_, Ctx>, query: String) -> R<Vec<SearchHit>> {
    ctx.client.search(query).await.map_err(ui)
}

#[tauri::command]
async fn diagnostics(ctx: State<'_, Ctx>) -> R<Diagnostics> {
    ctx.client.diagnostics().await.map_err(ui)
}

#[tauri::command]
fn notify(app: AppHandle, title: String, body: String) -> R<()> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| UiError {
            code: "notify".into(),
            message: e.to_string(),
        })
}

#[tauri::command]
fn open_link(app: AppHandle, url: String) -> R<()> {
    let ok = url.starts_with("https://") || url.starts_with("http://");
    if !ok {
        return Err(UiError {
            code: "invalid".into(),
            message: "Only web links can be opened.".into(),
        });
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| UiError {
            code: "open".into(),
            message: e.to_string(),
        })
}

#[tauri::command]
fn autostart_enabled(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: AppHandle, on: bool) -> R<()> {
    let m = app.autolaunch();
    let r = if on { m.enable() } else { m.disable() };
    r.map_err(|e| UiError {
        code: "autostart".into(),
        message: e.to_string(),
    })
}

#[tauri::command]
fn set_unread(app: AppHandle, count: u32) {
    if let Some(tray) = app.tray_by_id("main") {
        let tip = if count == 0 {
            "SpareLine".to_string()
        } else {
            format!("SpareLine — {count} unread")
        };
        let _ = tray.set_tooltip(Some(tip));
    }
    #[cfg(target_os = "macos")]
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_badge_count(if count == 0 { None } else { Some(count as i64) });
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sl_client=info".into()),
        )
        .init();

    let dir = sl_client_core::profile::default_dir();
    let keyring = KeyringStore::for_dir(&dir);
    let secrets: Arc<dyn SecretStore> = if keyring.probe() {
        Arc::new(keyring)
    } else {
        tracing::warn!("OS keychain unavailable; storing secrets in the data folder");
        let _ = std::fs::create_dir_all(&dir);
        Arc::new(FileStore::new(&dir))
    };
    let device_name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "This computer".into());
    let client = match Client::start(ClientOptions {
        dir,
        secrets,
        device_name,
        platform: std::env::consts::OS.into(),
        client_version: env!("CARGO_PKG_VERSION").into(),
        discovery: true,
    }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SpareLine could not start: {e:#}");
            std::process::exit(1);
        }
    };

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app)
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Ctx {
            client: client.clone(),
        })
        .setup(move |app| {
            // Forward engine events to the UI.
            let handle = app.handle().clone();
            let mut rx = client.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(ev) => {
                            let _ = handle.emit("sl://event", &ev);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "UI event stream lagged");
                            let _ = handle.emit("sl://resync", ());
                        }
                        Err(_) => break,
                    }
                }
            });

            // Tray: SpareLine sits quietly until it's needed.
            let open = MenuItem::with_id(app, "open", "Open SpareLine", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit SpareLine", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("SpareLine")
                .menu(&menu)
                .show_menu_on_left_click(false);
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.on_menu_event(|app, e| match e.id.as_ref() {
                "open" => show_main(app),
                "quit" => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|tray, e| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = e
                {
                    show_main(tray.app_handle());
                }
            })
            .build(app)?;

            if std::env::args().any(|a| a == "--hidden")
                && let Some(w) = app.get_webview_window("main")
            {
                let _ = w.hide();
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps SpareLine running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_state,
            app_info,
            discover,
            join,
            redeem,
            login,
            logout,
            reconnect_now,
            conversations,
            messages,
            send,
            edit,
            delete,
            react,
            mark_read,
            typing,
            start_chat,
            add_people,
            leave,
            directory,
            set_presence,
            set_display_name,
            change_password,
            read_receipts,
            set_read_receipts,
            search,
            diagnostics,
            notify,
            open_link,
            autostart_enabled,
            set_autostart,
            set_unread,
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("SpareLine stopped: {e}");
    }
}
