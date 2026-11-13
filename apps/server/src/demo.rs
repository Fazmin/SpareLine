//! Browser preview (`index.html?demo`, `?demo=setup`, `?demo=locked`): canned data so the
//! control panel can be reviewed without the desktop shell. Never used inside the app.

use serde_json::{Value, json};
use sl_proto::panel::*;
use sl_proto::*;
use uuid::Uuid;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n)
}

fn now() -> i64 {
    js_sys::Date::now() as i64
}

fn uref(n: u128, name: &str, un: &str) -> UserRef {
    UserRef {
        id: id(n),
        username: un.into(),
        display_name: name.into(),
    }
}

fn groups() -> Vec<GroupRow> {
    vec![
        GroupRow {
            id: id(900),
            name: "Operations".into(),
            description: "Ops and IT".into(),
            member_count: 18,
            created_at: now() - 40 * 86_400_000,
        },
        GroupRow {
            id: id(901),
            name: "Finance".into(),
            description: String::new(),
            member_count: 9,
            created_at: now() - 40 * 86_400_000,
        },
        GroupRow {
            id: id(902),
            name: "Leadership".into(),
            description: String::new(),
            member_count: 6,
            created_at: now() - 30 * 86_400_000,
        },
        GroupRow {
            id: id(903),
            name: "Everyone".into(),
            description: "All staff".into(),
            member_count: 47,
            created_at: now() - 40 * 86_400_000,
        },
    ]
}

fn gref(n: u128, name: &str) -> GroupRef {
    GroupRef {
        id: id(n),
        name: name.into(),
    }
}

fn users() -> Vec<UserRow> {
    let u = |n: u128,
             name: &str,
             un: &str,
             g: Vec<GroupRef>,
             online: bool,
             dev: u32,
             seen: i64,
             st: UserStatus| UserRow {
        id: id(n),
        username: un.into(),
        display_name: name.into(),
        status: st,
        groups: g,
        devices: dev,
        online,
        last_seen: Some(now() - seen * 60_000),
        created_at: now() - 30 * 86_400_000,
    };
    vec![
        u(
            1,
            "Alex Morgan",
            "alex.morgan",
            vec![gref(900, "Operations"), gref(903, "Everyone")],
            true,
            2,
            0,
            UserStatus::Active,
        ),
        u(
            6,
            "Chen Wei",
            "chen.wei",
            vec![gref(901, "Finance"), gref(903, "Everyone")],
            true,
            1,
            0,
            UserStatus::Active,
        ),
        u(
            4,
            "Jordan Lee",
            "jordan.lee",
            vec![
                gref(900, "Operations"),
                gref(902, "Leadership"),
                gref(903, "Everyone"),
            ],
            true,
            1,
            0,
            UserStatus::Active,
        ),
        u(
            5,
            "Maria Gonzalez",
            "maria.g",
            vec![gref(901, "Finance"), gref(903, "Everyone")],
            false,
            1,
            190,
            UserStatus::Active,
        ),
        u(
            2,
            "Priya Shah",
            "priya.shah",
            vec![gref(900, "Operations"), gref(903, "Everyone")],
            true,
            1,
            0,
            UserStatus::Active,
        ),
        u(
            3,
            "Sam Okafor",
            "sam.okafor",
            vec![
                gref(900, "Operations"),
                gref(901, "Finance"),
                gref(903, "Everyone"),
            ],
            false,
            1,
            12,
            UserStatus::Active,
        ),
        u(
            7,
            "Taylor Brooks",
            "taylor.b",
            vec![gref(903, "Everyone")],
            false,
            0,
            0,
            UserStatus::Active,
        ),
        u(
            8,
            "Robin Hart",
            "robin.hart",
            vec![],
            false,
            1,
            30 * 1440,
            UserStatus::Disabled,
        ),
    ]
}

fn health() -> Vec<HealthCheck> {
    let c = |id: &str, name: &str, st: CheckStatus, d: &str| HealthCheck {
        id: id.into(),
        name: name.into(),
        status: st,
        detail: d.into(),
        fix: None,
    };
    let mut fw = c(
        "firewall",
        "Windows Firewall",
        CheckStatus::Warn,
        "No SpareLine firewall rule found. If staff can't connect, allow TCP 7443 and mDNS (UDP 5353).",
    );
    fw.fix = Some(FixAction {
        id: "firewall".into(),
        label: "Add firewall rules".into(),
    });
    vec![
        c(
            "listener",
            "Accepting connections",
            CheckStatus::Ok,
            "Listening on port 7443",
        ),
        c(
            "network",
            "Network address",
            CheckStatus::Ok,
            "Clients can connect to 192.168.1.20:7443",
        ),
        fw,
        c(
            "mdns",
            "Automatic discovery (mDNS)",
            CheckStatus::Ok,
            "Advertising as \"Head office (01a1d5)\"",
        ),
        c(
            "tls",
            "Server identity (TLS)",
            CheckStatus::Ok,
            "Fingerprint E0AE E89D F901 74CD 9568 C274 8447 8705 · valid for 3650 more days",
        ),
        c(
            "database",
            "Database",
            CheckStatus::Ok,
            "Integrity ok · 4.2 MB · WAL 312.0 KB",
        ),
        c(
            "disk",
            "Disk space",
            CheckStatus::Ok,
            "212.4 GB free of 476.3 GB",
        ),
        c("clock", "System clock", CheckStatus::Ok, "Looks right"),
        c(
            "backup",
            "Backups",
            CheckStatus::Ok,
            "Last backup 9 h ago (spareline-1763011200000.db)",
        ),
    ]
}

fn overview() -> Value {
    let t = now();
    let minute = t - t.rem_euclid(60_000);
    let series: Vec<StatPoint> = (0..1440)
        .map(|i| {
            let ts = minute - (1439 - i) * 60_000;
            let k = i as f64;
            let msgs = if i > 1380 {
                ((k * 1.7).sin().abs() * 30.0 + (k - 1380.0) * 0.6) as u32
            } else {
                ((k * 0.13).sin().abs() * 3.0) as u32
            };
            let conns = if i > 1385 {
                (24.0 + (k * 0.3).sin() * 4.0 + (k - 1385.0) * 0.12) as u32
            } else {
                6 + ((k * 0.05).sin().abs() * 3.0) as u32
            };
            StatPoint {
                ts,
                connections: conns,
                messages: msgs,
                auth_failures: 0,
                errors: 0,
            }
        })
        .collect();
    let o = Overview {
        status: ServerStatus::Running,
        server_name: "Head office".into(),
        addresses: vec!["192.168.1.20:7443".into()],
        port: 7443,
        fingerprint: "E0AE E89D F901 74CD 9568 C274 8447 8705".into(),
        started_at: t - 9 * 86_400_000 - 4 * 3_600_000,
        users_total: 47,
        users_online: 31,
        devices_total: 52,
        devices_ready: 49,
        connections: 33,
        messages_last_hour: series[1380..].iter().map(|p| p.messages as u64).sum(),
        messages_last_24h: series.iter().map(|p| p.messages as u64).sum(),
        series,
        system: SystemStats {
            cpu_percent: 3.4,
            mem_used: 9_800_000_000,
            mem_total: 16_000_000_000,
            process_mem: 42_000_000,
            disk_free: 212_400_000_000,
            disk_total: 476_300_000_000,
            db_size: 4_400_000,
        },
        cert_not_after: t + 3650 * 86_400_000,
        health: health(),
        debug_mode_until: None,
        mdns_active: true,
        active_broadcasts: 1,
    };
    let mut v = serde_json::to_value(o).unwrap_or_default();
    v["start_error"] = Value::Null;
    v
}

fn detail(uid: Uuid) -> UserDetail {
    let user = users()
        .into_iter()
        .find(|u| u.id == uid)
        .unwrap_or_else(|| users().remove(0));
    UserDetail {
        devices: vec![
            DeviceRow {
                id: id(300),
                name: "ALEX-LAPTOP".into(),
                platform: "windows".into(),
                client_version: "0.1.0".into(),
                created_at: now() - 20 * 86_400_000,
                last_seen: Some(now()),
                revoked: false,
                online: user.online,
            },
            DeviceRow {
                id: id(301),
                name: "Alex's MacBook".into(),
                platform: "macos".into(),
                client_version: "0.1.0".into(),
                created_at: now() - 3 * 86_400_000,
                last_seen: Some(now() - 2 * 86_400_000),
                revoked: false,
                online: false,
            },
        ],
        sessions: vec![SessionRow {
            id: "a3f9…".into(),
            device_id: id(300),
            created_at: now() - 20 * 86_400_000,
            last_used: now() - 60_000,
            expires_at: now() + 30 * 86_400_000,
            ip: "192.168.1.54".into(),
        }],
        recent_events: vec![
            ConnectionEvent {
                ts: now() - 40 * 60_000,
                ip: "192.168.1.54".into(),
                user: Some(uref(1, &user.display_name, &user.username)),
                device_id: Some(id(300)),
                kind: ConnEventKind::Connected,
                detail: "windows 0.1.0".into(),
            },
            ConnectionEvent {
                ts: now() - 41 * 60_000,
                ip: "192.168.1.54".into(),
                user: None,
                device_id: None,
                kind: ConnEventKind::TlsFailed,
                detail: "Client disconnected during the TLS handshake".into(),
            },
            ConnectionEvent {
                ts: now() - 2 * 86_400_000,
                ip: "10.8.0.12".into(),
                user: Some(uref(1, &user.display_name, &user.username)),
                device_id: Some(id(301)),
                kind: ConnEventKind::AuthFailed,
                detail: "wrong password".into(),
            },
        ],
        health_reports: vec![HealthReportRow {
            received_at: now() - 39 * 60_000,
            device_id: id(300),
            report: ClientHealthReport {
                client_version: "0.1.0".into(),
                platform: "windows".into(),
                window_start: now() - 44 * 60_000,
                window_end: now() - 41 * 60_000,
                failures: vec![("tcp_timeout".into(), 3), ("tls_handshake".into(), 1)],
                last_error: "connection timed out (server off, wrong address, or firewall)".into(),
                discovery_method: "mdns".into(),
            },
        }],
        user,
    }
}

fn logs() -> Vec<LogEntry> {
    let lines = [
        (
            LogLevel::Info,
            "sl_server",
            "SpareLine Server is running",
            "local=0.0.0.0:7443 ",
        ),
        (
            LogLevel::Info,
            "sl_server::mdns",
            "advertising on the LAN",
            "instance=\"Head office (01a1d5)\" port=7443 ",
        ),
        (
            LogLevel::Info,
            "sl_server::ws",
            "connected",
            "user=priya.shah device=PRIYA-PC peer=192.168.1.61:52144 ",
        ),
        (
            LogLevel::Info,
            "sl_server::ws",
            "connection problem",
            "ip=192.168.1.54 kind=tls_failed detail=Client disconnected during the TLS handshake ",
        ),
        (
            LogLevel::Info,
            "sl_server::ws",
            "connected",
            "user=alex.morgan device=ALEX-LAPTOP peer=192.168.1.54:50122 ",
        ),
        (
            LogLevel::Info,
            "sl_server::admin",
            "admin action",
            "action=broadcast.create target=Our usual chat is down. Use SpareLine for now. ",
        ),
        (
            LogLevel::Warn,
            "sl_server::ws",
            "client too slow; disconnecting",
            "conn=88 user=robin.hart ",
        ),
        (
            LogLevel::Info,
            "sl_server",
            "backup written",
            "file=spareline-1763011200000.db size=4404224 ",
        ),
    ];
    lines
        .iter()
        .enumerate()
        .map(|(i, (l, t, m, f))| LogEntry {
            seq: i as u64 + 1,
            ts: now() - (60 - i as i64 * 6) * 60_000,
            level: *l,
            target: (*t).into(),
            message: (*m).into(),
            fields: (*f).into(),
        })
        .collect()
}

pub fn install(mode: &str) {
    preset_theme();
    let mode = mode.to_string();
    sl_ui::ipc::set_mock(move |cmd, args| {
        let v: Value = match cmd {
            "panel_state" => json!({
                "needs_setup": mode == "setup",
                "unlocked": mode != "locked",
                "server_name": "Head office",
                "suggested_name": "SpareLine on OFFICE-PC",
            }),
            "overview" => overview(),
            "users" => serde_json::to_value(users()).unwrap_or_default(),
            "user_detail" => {
                let uid: Uuid = args
                    .get("id")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                serde_json::to_value(detail(uid)).unwrap_or_default()
            }
            "groups" => serde_json::to_value(groups()).unwrap_or_default(),
            "group_detail" => {
                let gid: Uuid = args
                    .get("id")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let g = groups()
                    .into_iter()
                    .find(|g| g.id == gid)
                    .unwrap_or_else(|| groups().remove(0));
                serde_json::to_value(GroupDetail {
                    group: g,
                    members: vec![
                        uref(1, "Alex Morgan", "alex.morgan"),
                        uref(2, "Priya Shah", "priya.shah"),
                        uref(4, "Jordan Lee", "jordan.lee"),
                    ],
                })
                .unwrap_or_default()
            }
            "invites" => serde_json::to_value(vec![
                InviteRow {
                    id: id(800),
                    kind: InviteKind::NewUser,
                    groups: vec![gref(903, "Everyone"), gref(900, "Operations")],
                    target_user: None,
                    used_by: None,
                    note: "New starters, December".into(),
                    created_at: now() - 86_400_000,
                    expires_at: now() + 6 * 86_400_000,
                    status: InviteStatus::Pending,
                },
                InviteRow {
                    id: id(801),
                    kind: InviteKind::Setup,
                    groups: vec![],
                    target_user: Some(uref(3, "Sam Okafor", "sam.okafor")),
                    used_by: None,
                    note: String::new(),
                    created_at: now() - 3_600_000,
                    expires_at: now() + 23 * 3_600_000,
                    status: InviteStatus::Pending,
                },
            ])
            .unwrap_or_default(),
            "broadcasts" => serde_json::to_value(vec![BroadcastRow {
                broadcast: Broadcast {
                    id: id(700),
                    title: "Our usual chat is down. Use SpareLine for now.".into(),
                    body: "Our cloud chat and email are unavailable this morning.".into(),
                    severity: Severity::Warning,
                    created_at: now() - 42 * 60_000,
                    expires_at: None,
                },
                active: true,
                ended_at: None,
            }])
            .unwrap_or_default(),
            "connections" => serde_json::to_value(vec![
                ConnectionRow {
                    conn_id: 1,
                    user: uref(1, "Alex Morgan", "alex.morgan"),
                    device_id: id(300),
                    device_name: "ALEX-LAPTOP".into(),
                    ip: "192.168.1.54".into(),
                    connected_at: now() - 40 * 60_000,
                    rtt_ms: Some(3),
                    client_version: "0.1.0".into(),
                    platform: "windows".into(),
                },
                ConnectionRow {
                    conn_id: 2,
                    user: uref(2, "Priya Shah", "priya.shah"),
                    device_id: id(302),
                    device_name: "PRIYA-PC".into(),
                    ip: "192.168.1.61".into(),
                    connected_at: now() - 55 * 60_000,
                    rtt_ms: Some(5),
                    client_version: "0.1.0".into(),
                    platform: "windows".into(),
                },
                ConnectionRow {
                    conn_id: 3,
                    user: uref(6, "Chen Wei", "chen.wei"),
                    device_id: id(303),
                    device_name: "Chen's MacBook Air".into(),
                    ip: "192.168.1.77".into(),
                    connected_at: now() - 12 * 60_000,
                    rtt_ms: Some(11),
                    client_version: "0.1.0".into(),
                    platform: "macos".into(),
                },
            ])
            .unwrap_or_default(),
            "connection_events" => {
                serde_json::to_value(detail(id(1)).recent_events).unwrap_or_default()
            }
            "logs" => {
                let after = args
                    .get("query")
                    .and_then(|q| q.get("after_seq"))
                    .and_then(Value::as_u64);
                if after.is_some() {
                    json!([])
                } else {
                    serde_json::to_value(logs()).unwrap_or_default()
                }
            }
            "health" => serde_json::to_value(health()).unwrap_or_default(),
            "self_test" => json!([]),
            "audit" => serde_json::to_value(vec![
                AuditRow {
                    ts: now() - 42 * 60_000,
                    action: "broadcast.create".into(),
                    target: "Our usual chat is down. Use SpareLine for now.".into(),
                    detail: "severity=warning".into(),
                },
                AuditRow {
                    ts: now() - 86_400_000,
                    action: "invite.create".into(),
                    target: "5 invite(s)".into(),
                    detail: "Everyone, Operations".into(),
                },
            ])
            .unwrap_or_default(),
            "settings" => serde_json::to_value(ServerSettings {
                server_name: "Head office".into(),
                port: 7443,
                envelope_retention_days: 30,
                log_retention_days: 14,
                backup_keep: 7,
                session_idle_days: 30,
            })
            .unwrap_or_default(),
            "backups" => serde_json::to_value(vec![BackupRow {
                file_name: "spareline-1763011200000.db".into(),
                created_at: now() - 9 * 3_600_000,
                size: 4_404_224,
            }])
            .unwrap_or_default(),
            "app_info" => {
                json!({ "version": "0.1.0", "data_dir": "C:\\Users\\it-admin\\AppData\\Local\\SpareLine\\Server", "platform": "windows" })
            }
            "autostart_enabled" => json!(true),
            "setup" => json!({ "recovery_key": "RK-RH6S-CYJN-ZBSG-QMX0-XHBT-93EH" }),
            _ => Value::Null,
        };
        Ok(v)
    });
}

/// Preview only: `&theme=light|dark` presets the stored appearance before the app mounts.
fn preset_theme() {
    let Some(w) = web_sys::window() else { return };
    let search = w.location().search().unwrap_or_default();
    let theme = search
        .trim_start_matches('?')
        .split('&')
        .find_map(|p| p.strip_prefix("theme="))
        .map(str::to_string);
    if let (Some(t), Ok(Some(store))) = (theme, w.local_storage()) {
        let _ = store.set_item(
            sl_ui::prefs::STORAGE_KEY,
            &format!("{{\"theme\":\"{t}\",\"scale\":1}}"),
        );
        if let Some(el) = w.document().and_then(|d| d.document_element()) {
            let _ = el.set_attribute("data-theme", &t);
        }
    }
}
