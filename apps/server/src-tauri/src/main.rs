//! SpareLine Server: a tray app that runs the server engine and hosts the admin
//! control panel. The panel talks to the engine in-process; nothing admin-related is
//! reachable over the network.
//!
//! Headless: `spareline-server --headless [--data-dir DIR]`
//! (first run: set `SPARELINE_ADMIN_PASSWORD` to complete setup without the panel).
//!
//! Taking over from a failed server: `spareline-server --headless --restore COPY`
//! with `SPARELINE_STANDBY_PASSWORD` set, on a fresh install or an empty `--data-dir`.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use sl_proto::client_ipc::UiError;
use sl_proto::panel::*;
use sl_proto::{ErrorCode, GroupId, UserId};
use sl_server::{AppError, Engine, EngineOptions, Paths};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_dialog::DialogExt as _;
use tauri_plugin_opener::OpenerExt as _;
use uuid::Uuid;

const LOCK_AFTER: Duration = Duration::from_secs(15 * 60);

struct Panel {
    engine: Engine,
    /// Last activity while unlocked; `None` = locked.
    unlocked: Mutex<Option<Instant>>,
    start_error: Mutex<Option<String>>,
}

type R<T> = Result<T, UiError>;

fn ui(e: AppError) -> UiError {
    let api = e.into_api();
    let code = serde_json::to_value(api.code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "error".into());
    let message = if api.detail.is_empty() || matches!(api.code, ErrorCode::Internal) {
        api.user_message().to_string()
    } else {
        // Admin-facing: the detail is more useful than the generic text.
        let d = api.detail.clone();
        let mut chars = d.chars();
        match chars.next() {
            Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
            None => api.user_message().to_string(),
        }
    };
    UiError { code, message }
}

fn err(code: &str, message: impl Into<String>) -> UiError {
    UiError {
        code: code.into(),
        message: message.into(),
    }
}

impl Panel {
    fn guard(&self) -> R<()> {
        let mut g = self.unlocked.lock();
        match *g {
            Some(t) if t.elapsed() < LOCK_AFTER => {
                *g = Some(Instant::now());
                Ok(())
            }
            _ => {
                *g = None;
                Err(err(
                    "locked",
                    "The control panel is locked. Enter the admin password.",
                ))
            }
        }
    }
}

// ---- gate ------------------------------------------------------------------------------

#[tauri::command]
fn panel_state(p: State<'_, Panel>) -> PanelState {
    let unlocked = p.unlocked.lock().is_some_and(|t| t.elapsed() < LOCK_AFTER);
    PanelState {
        needs_setup: p.engine.needs_setup(),
        unlocked,
        server_name: p.engine.config().server_name,
        suggested_name: sl_server::config::default_server_name(),
    }
}

#[tauri::command]
async fn setup(p: State<'_, Panel>, input: SetupInput) -> R<SetupResult> {
    let key = p
        .engine
        .setup(&input.server_name, &input.admin_password)
        .await
        .map_err(ui)?;
    if let Err(e) = p.engine.start().await {
        *p.start_error.lock() = Some(e.to_string());
    }
    *p.unlocked.lock() = Some(Instant::now());
    Ok(SetupResult { recovery_key: key })
}

#[tauri::command]
async fn unlock(p: State<'_, Panel>, password: String) -> R<()> {
    if p.engine.verify_admin(&password).await {
        *p.unlocked.lock() = Some(Instant::now());
        p.engine.audit("admin.unlock", "", "");
        Ok(())
    } else {
        tokio::time::sleep(Duration::from_millis(600)).await;
        Err(err("invalid_credentials", "That password isn't right."))
    }
}

#[tauri::command]
fn lock(p: State<'_, Panel>) {
    *p.unlocked.lock() = None;
}

#[tauri::command]
async fn reset_password(p: State<'_, Panel>, recovery: String, new: String) -> R<String> {
    let key = p
        .engine
        .admin()
        .reset_admin_password(recovery, new)
        .await
        .map_err(ui)?;
    *p.unlocked.lock() = Some(Instant::now());
    Ok(key)
}

#[tauri::command]
async fn change_admin_password(p: State<'_, Panel>, current: String, new: String) -> R<()> {
    p.guard()?;
    p.engine
        .admin()
        .change_admin_password(current, new)
        .await
        .map_err(ui)
}

// ---- overview & monitoring -------------------------------------------------------------

#[derive(Serialize)]
struct OverviewOut {
    #[serde(flatten)]
    overview: Overview,
    start_error: Option<String>,
}

#[tauri::command]
async fn overview(p: State<'_, Panel>) -> R<OverviewOut> {
    p.guard()?;
    let o = p.engine.admin().overview().await.map_err(ui)?;
    Ok(OverviewOut {
        overview: o,
        start_error: p.start_error.lock().clone(),
    })
}

#[tauri::command]
fn logs(p: State<'_, Panel>, query: LogQuery) -> R<Vec<LogEntry>> {
    p.guard()?;
    Ok(p.engine.admin().logs(&query))
}

#[tauri::command]
fn connections(p: State<'_, Panel>) -> R<Vec<ConnectionRow>> {
    p.guard()?;
    Ok(p.engine.admin().connections())
}

#[tauri::command]
async fn connection_events(
    p: State<'_, Panel>,
    user: Option<UserId>,
    limit: u32,
) -> R<Vec<ConnectionEvent>> {
    p.guard()?;
    p.engine
        .admin()
        .connection_events(user, limit)
        .await
        .map_err(ui)
}

#[tauri::command]
async fn health(p: State<'_, Panel>) -> R<Vec<HealthCheck>> {
    p.guard()?;
    Ok(p.engine.admin().health().await)
}

#[tauri::command]
async fn run_fix(p: State<'_, Panel>, id: String) -> R<String> {
    p.guard()?;
    p.engine.admin().run_fix(&id).await.map_err(ui)
}

#[tauri::command]
async fn self_test(p: State<'_, Panel>) -> R<Vec<SelfTestStep>> {
    p.guard()?;
    Ok(p.engine.admin().self_test().await)
}

#[tauri::command]
fn set_debug_mode(p: State<'_, Panel>, minutes: u32) -> R<()> {
    p.guard()?;
    p.engine.admin().set_debug_mode(minutes);
    Ok(())
}

#[tauri::command]
async fn audit(p: State<'_, Panel>, limit: u32) -> R<Vec<AuditRow>> {
    p.guard()?;
    p.engine.admin().audit_log(limit).await.map_err(ui)
}

#[tauri::command]
async fn diagnostics_report(app: AppHandle, p: State<'_, Panel>) -> R<String> {
    p.guard()?;
    let path = p.engine.admin().diagnostics_report().await.map_err(ui)?;
    let _ = app.opener().reveal_item_in_dir(&path);
    Ok(path.to_string_lossy().to_string())
}

// ---- users -----------------------------------------------------------------------------

#[tauri::command]
async fn users(p: State<'_, Panel>) -> R<Vec<UserRow>> {
    p.guard()?;
    p.engine.admin().users().await.map_err(ui)
}

#[tauri::command]
async fn user_detail(p: State<'_, Panel>, id: UserId) -> R<UserDetail> {
    p.guard()?;
    p.engine.admin().user_detail(id).await.map_err(ui)
}

#[tauri::command]
async fn set_user_disabled(p: State<'_, Panel>, id: UserId, disabled: bool) -> R<()> {
    p.guard()?;
    p.engine
        .admin()
        .set_user_disabled(id, disabled)
        .await
        .map_err(ui)
}

#[tauri::command]
async fn delete_user(p: State<'_, Panel>, id: UserId) -> R<()> {
    p.guard()?;
    p.engine.admin().delete_user(id).await.map_err(ui)
}

#[tauri::command]
async fn revoke_device(p: State<'_, Panel>, id: Uuid) -> R<()> {
    p.guard()?;
    p.engine.admin().revoke_device(id).await.map_err(ui)
}

#[tauri::command]
async fn revoke_session(p: State<'_, Panel>, id: String) -> R<()> {
    p.guard()?;
    p.engine.admin().revoke_session(id).await.map_err(ui)
}

#[tauri::command]
async fn sign_out_everywhere(p: State<'_, Panel>, id: UserId) -> R<()> {
    p.guard()?;
    p.engine.admin().sign_out_everywhere(id).await.map_err(ui)
}

#[tauri::command]
async fn set_user_groups(p: State<'_, Panel>, id: UserId, groups: Vec<GroupId>) -> R<()> {
    p.guard()?;
    p.engine
        .admin()
        .set_user_groups(id, groups)
        .await
        .map_err(ui)
}

#[tauri::command]
async fn create_setup_code(p: State<'_, Panel>, id: UserId, expires: i64) -> R<CreatedInvite> {
    p.guard()?;
    p.engine
        .admin()
        .create_setup_code(id, expires)
        .await
        .map_err(ui)
}

// ---- groups ----------------------------------------------------------------------------

#[tauri::command]
async fn groups(p: State<'_, Panel>) -> R<Vec<GroupRow>> {
    p.guard()?;
    p.engine.admin().groups().await.map_err(ui)
}

#[tauri::command]
async fn group_detail(p: State<'_, Panel>, id: GroupId) -> R<GroupDetail> {
    p.guard()?;
    p.engine.admin().group_detail(id).await.map_err(ui)
}

#[tauri::command]
async fn create_group(p: State<'_, Panel>, name: String, description: String) -> R<GroupRow> {
    p.guard()?;
    p.engine
        .admin()
        .create_group(name, description)
        .await
        .map_err(ui)
}

#[tauri::command]
async fn update_group(
    p: State<'_, Panel>,
    id: GroupId,
    name: String,
    description: String,
) -> R<()> {
    p.guard()?;
    p.engine
        .admin()
        .update_group(id, name, description)
        .await
        .map_err(ui)
}

#[tauri::command]
async fn delete_group(p: State<'_, Panel>, id: GroupId) -> R<()> {
    p.guard()?;
    p.engine.admin().delete_group(id).await.map_err(ui)
}

#[tauri::command]
async fn set_group_members(p: State<'_, Panel>, id: GroupId, users: Vec<UserId>) -> R<()> {
    p.guard()?;
    p.engine
        .admin()
        .set_group_members(id, users)
        .await
        .map_err(ui)
}

// ---- invites & broadcasts --------------------------------------------------------------

#[tauri::command]
async fn invites(p: State<'_, Panel>) -> R<Vec<InviteRow>> {
    p.guard()?;
    p.engine.admin().invites().await.map_err(ui)
}

#[tauri::command]
async fn create_invites(p: State<'_, Panel>, input: CreateInvitesInput) -> R<Vec<CreatedInvite>> {
    p.guard()?;
    p.engine.admin().create_invites(input).await.map_err(ui)
}

#[tauri::command]
async fn revoke_invite(p: State<'_, Panel>, id: Uuid) -> R<()> {
    p.guard()?;
    p.engine.admin().revoke_invite(id).await.map_err(ui)
}

#[tauri::command]
async fn broadcasts(p: State<'_, Panel>) -> R<Vec<BroadcastRow>> {
    p.guard()?;
    p.engine.admin().broadcasts().await.map_err(ui)
}

#[tauri::command]
async fn create_broadcast(p: State<'_, Panel>, input: BroadcastInput) -> R<()> {
    p.guard()?;
    p.engine.admin().create_broadcast(input).await.map_err(ui)
}

#[tauri::command]
async fn end_broadcast(p: State<'_, Panel>, id: Uuid) -> R<()> {
    p.guard()?;
    p.engine.admin().end_broadcast(id).await.map_err(ui)
}

// ---- settings & backups ----------------------------------------------------------------

#[tauri::command]
fn settings(p: State<'_, Panel>) -> R<ServerSettings> {
    p.guard()?;
    Ok(p.engine.admin().settings())
}

#[tauri::command]
async fn update_settings(p: State<'_, Panel>, input: ServerSettings) -> R<bool> {
    p.guard()?;
    p.engine.admin().update_settings(input).await.map_err(ui)
}

#[tauri::command]
async fn restart_server(p: State<'_, Panel>) -> R<()> {
    p.guard()?;
    p.engine.stop().await;
    match p.engine.start().await {
        Ok(_) => {
            *p.start_error.lock() = None;
            Ok(())
        }
        Err(e) => {
            *p.start_error.lock() = Some(e.to_string());
            Err(err("start", e.to_string()))
        }
    }
}

#[tauri::command]
fn backups(p: State<'_, Panel>) -> R<Vec<BackupRow>> {
    p.guard()?;
    Ok(p.engine.admin().backups())
}

#[tauri::command]
async fn backup_now(p: State<'_, Panel>) -> R<BackupRow> {
    p.guard()?;
    p.engine.admin().backup_now().await.map_err(ui)
}

// ---- standby copies -------------------------------------------------------------------

#[tauri::command]
fn standby_status(p: State<'_, Panel>) -> R<StandbyStatus> {
    p.guard()?;
    Ok(p.engine.admin().standby_status())
}

#[tauri::command]
async fn standby_setup(p: State<'_, Panel>, input: StandbySetupInput) -> R<StandbyStatus> {
    p.guard()?;
    p.engine.admin().standby_setup(input).await.map_err(ui)
}

#[tauri::command]
async fn standby_copy_now(p: State<'_, Panel>) -> R<StandbyCopyRow> {
    p.guard()?;
    p.engine.admin().standby_copy_now().await.map_err(ui)
}

#[tauri::command]
fn standby_turn_off(p: State<'_, Panel>) -> R<()> {
    p.guard()?;
    p.engine.admin().standby_turn_off().map_err(ui)
}

/// Choose the folder for standby copies.
#[tauri::command]
async fn pick_folder(app: AppHandle, p: State<'_, Panel>) -> R<Option<String>> {
    p.guard()?;
    Ok(app
        .dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|f| f.into_path().ok())
        .map(|f| f.to_string_lossy().to_string()))
}

/// Restoring is offered only before setup, on a freshly installed server.
fn restore_allowed(p: &Panel) -> R<()> {
    if p.engine.needs_setup() {
        Ok(())
    } else {
        Err(err("already_set_up", "This server is already set up."))
    }
}

#[tauri::command]
async fn pick_standby_copy(app: AppHandle, p: State<'_, Panel>) -> R<Option<String>> {
    restore_allowed(&p)?;
    Ok(app
        .dialog()
        .file()
        .add_filter("SpareLine standby copy", &["slstandby"])
        .blocking_pick_file()
        .and_then(|f| f.into_path().ok())
        .map(|f| f.to_string_lossy().to_string()))
}

/// Check a copy's password and describe it, without changing anything.
#[tauri::command]
async fn standby_inspect(p: State<'_, Panel>, path: String, password: String) -> R<StandbyInfo> {
    restore_allowed(&p)?;
    tokio::task::spawn_blocking(move || sl_server::standby::inspect(Path::new(&path), &password))
        .await
        .map_err(|e| err("internal", e.to_string()))?
        .map_err(ui)
}

#[tauri::command]
async fn standby_restore(
    app: AppHandle,
    p: State<'_, Panel>,
    path: String,
    password: String,
) -> R<RestoreResult> {
    restore_allowed(&p)?;
    let info = p
        .engine
        .restore_standby(Path::new(&path), &password)
        .await
        .map_err(ui)?;
    if let Err(e) = p.engine.start().await {
        *p.start_error.lock() = Some(e.to_string());
    }
    *p.unlocked.lock() = Some(Instant::now());
    let _ = app.autolaunch().enable();
    Ok(RestoreResult {
        info,
        addresses: sl_server::health::lan_addresses(),
    })
}

#[tauri::command]
fn open_data_folder(app: AppHandle, p: State<'_, Panel>) -> R<()> {
    p.guard()?;
    app.opener()
        .open_path(p.engine.paths().root.to_string_lossy(), None::<&str>)
        .map_err(|e| err("open", e.to_string()))
}

/// Save text (CSV of invites, printable invite cards) through a native save dialog.
#[tauri::command]
async fn save_text(
    app: AppHandle,
    p: State<'_, Panel>,
    filename: String,
    content: String,
) -> R<Option<String>> {
    p.guard()?;
    let ext = filename.rsplit('.').next().unwrap_or("txt").to_string();
    let picked = app
        .dialog()
        .file()
        .set_file_name(&filename)
        .add_filter(ext.to_uppercase(), &[ext.as_str()])
        .blocking_save_file();
    let Some(path) = picked.and_then(|f| f.into_path().ok()) else {
        return Ok(None);
    };
    std::fs::write(&path, content).map_err(|e| err("save", e.to_string()))?;
    Ok(Some(path.to_string_lossy().to_string()))
}

#[tauri::command]
fn autostart_enabled(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: AppHandle, on: bool) -> R<()> {
    let m = app.autolaunch();
    let r = if on { m.enable() } else { m.disable() };
    r.map_err(|e| err("autostart", e.to_string()))
}

#[derive(Serialize)]
struct AppInfo {
    version: String,
    data_dir: String,
    platform: String,
}

#[tauri::command]
fn app_info(p: State<'_, Panel>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        data_dir: p.engine.paths().root.to_string_lossy().into(),
        platform: std::env::consts::OS.into(),
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data_dir = arg_value(&args, "--data-dir")
        .map(PathBuf::from)
        .unwrap_or_else(Paths::default_root);
    let headless = args.iter().any(|a| a == "--headless");
    let paths = Paths::new(&data_dir);
    let (telemetry, _guard) =
        match sl_server::telemetry::init(&paths, 14, headless || cfg!(debug_assertions)) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("could not start logging: {e}");
                std::process::exit(1);
            }
        };
    let engine = match Engine::open(EngineOptions::new(&data_dir, telemetry)) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "could not open the server data folder");
            eprintln!("SpareLine Server could not start: {e:#}");
            std::process::exit(1);
        }
    };

    if headless {
        run_headless(engine, arg_value(&args, "--restore"));
        return;
    }

    let start_engine = engine.clone();
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app)
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Panel {
            engine,
            unlocked: Mutex::new(None),
            start_error: Mutex::new(None),
        })
        .setup(move |app| {
            // "Just switch it on": serve as soon as the admin account exists.
            if !start_engine.needs_setup() {
                let e = start_engine.clone();
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = e.start().await {
                        tracing::error!(error = %err, "server failed to start");
                        if let Some(p) = handle.try_state::<Panel>() {
                            *p.start_error.lock() = Some(err.to_string());
                        }
                    }
                });
                // Enable start-at-login by default once set up (admins can turn it off).
                let _ = app.autolaunch().enable();
            }
            let open = MenuItem::with_id(app, "open", "Open control panel", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Stop server and quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("SpareLine Server")
                .menu(&menu)
                .show_menu_on_left_click(false);
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.on_menu_event(|app, e| match e.id.as_ref() {
                "open" => show_main(app),
                "quit" => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Some(p) = app.try_state::<Panel>() {
                            p.engine.stop().await;
                        }
                        app.exit(0);
                    });
                }
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
            // Closing the panel keeps the server running in the tray, and locks the panel.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                if let Some(p) = window.try_state::<Panel>() {
                    *p.unlocked.lock() = None;
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            panel_state,
            setup,
            unlock,
            lock,
            reset_password,
            change_admin_password,
            overview,
            logs,
            connections,
            connection_events,
            health,
            run_fix,
            self_test,
            set_debug_mode,
            audit,
            diagnostics_report,
            users,
            user_detail,
            set_user_disabled,
            delete_user,
            revoke_device,
            revoke_session,
            sign_out_everywhere,
            set_user_groups,
            create_setup_code,
            groups,
            group_detail,
            create_group,
            update_group,
            delete_group,
            set_group_members,
            invites,
            create_invites,
            revoke_invite,
            broadcasts,
            create_broadcast,
            end_broadcast,
            settings,
            update_settings,
            restart_server,
            backups,
            backup_now,
            standby_status,
            standby_setup,
            standby_copy_now,
            standby_turn_off,
            pick_folder,
            pick_standby_copy,
            standby_inspect,
            standby_restore,
            open_data_folder,
            save_text,
            autostart_enabled,
            set_autostart,
            app_info,
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("SpareLine Server stopped: {e}");
    }
}

fn run_headless(engine: Engine, restore: Option<String>) {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("could not start runtime: {e}");
            std::process::exit(1);
        }
    };
    rt.block_on(async move {
        if let Some(copy) = restore {
            if !engine.needs_setup() {
                eprintln!("This data folder is already set up. To restore a standby copy, use a fresh install or an empty --data-dir.");
                std::process::exit(2);
            }
            let Ok(pw) = std::env::var("SPARELINE_STANDBY_PASSWORD") else {
                eprintln!("Set SPARELINE_STANDBY_PASSWORD to the standby password to restore a copy.");
                std::process::exit(2);
            };
            match engine.restore_standby(Path::new(&copy), &pw).await {
                Ok(info) => {
                    println!(
                        "Restored {} from the standby copy taken {}.",
                        info.server_name,
                        sl_server::standby::utc_display(info.taken_at)
                    );
                    if !info.addresses.is_empty() {
                        println!("The old server's address was {}.", info.addresses.join(", "));
                    }
                    println!("Keep the old server switched off: two servers with the same identity would confuse staff apps.");
                }
                Err(e) => {
                    eprintln!("Restore failed: {e}");
                    std::process::exit(2);
                }
            }
        }
        if engine.needs_setup() {
            match std::env::var("SPARELINE_ADMIN_PASSWORD") {
                Ok(pw) => match engine.setup(&sl_server::config::default_server_name(), &pw).await {
                    Ok(key) => println!("Setup complete. Recovery key (store it safely): {key}"),
                    Err(e) => {
                        eprintln!("Setup failed: {e}");
                        std::process::exit(2);
                    }
                },
                Err(_) => {
                    eprintln!("First run: set SPARELINE_ADMIN_PASSWORD (10+ characters) or open the control panel once.");
                    std::process::exit(2);
                }
            }
        }
        match engine.start().await {
            Ok(addr) => {
                println!("SpareLine Server listening on {addr}");
                println!("Fingerprint: {}", engine.tls().fingerprint);
                for ip in sl_net_ips() {
                    println!("  Clients can connect to {ip}:{}", addr.port());
                }
            }
            Err(e) => {
                eprintln!("Could not start: {e:#}");
                std::process::exit(1);
            }
        }
        let _ = tokio::signal::ctrl_c().await;
        engine.stop().await;
    });
}

fn sl_net_ips() -> Vec<String> {
    // The engine exposes LAN addresses through the overview; reuse the same helper.
    sl_server::health::lan_addresses()
}
