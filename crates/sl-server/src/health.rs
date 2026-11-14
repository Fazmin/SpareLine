//! Health checks, one-click fixes and the end-to-end self-test.

use std::time::{Duration, Instant};

use futures_util::StreamExt;
use sl_proto::panel::{CheckStatus, FixAction, HealthCheck, SelfTestStep, SystemStats};
use sl_proto::wire::ServerFrame;

use crate::Engine;
use crate::error::{AppResult, api_detail};

const FIREWALL_RULE_TCP: &str = "SpareLine Server (TCP)";
const FIREWALL_RULE_MDNS: &str = "SpareLine Server (mDNS)";

fn check(id: &str, name: &str, status: CheckStatus, detail: impl Into<String>) -> HealthCheck {
    HealthCheck {
        id: id.into(),
        name: name.into(),
        status,
        detail: detail.into(),
        fix: None,
    }
}

pub async fn run_checks(engine: &Engine) -> Vec<HealthCheck> {
    let mut out = Vec::new();
    let port = engine.config().port;

    // Listening.
    match engine.local_addr() {
        Some(addr) => {
            let probe = tokio::time::timeout(
                Duration::from_secs(1),
                tokio::net::TcpStream::connect(("127.0.0.1", addr.port())),
            )
            .await;
            match probe {
                Ok(Ok(_)) => out.push(check(
                    "listener",
                    "Accepting connections",
                    CheckStatus::Ok,
                    format!("Listening on port {}", addr.port()),
                )),
                _ => out.push(check(
                    "listener",
                    "Accepting connections",
                    CheckStatus::Fail,
                    "The port is bound but not answering locally",
                )),
            }
        }
        None => out.push(check(
            "listener",
            "Accepting connections",
            CheckStatus::Fail,
            "The server is stopped",
        )),
    }

    // Network address.
    let ips = sl_net::lan_ipv4();
    if ips.is_empty() {
        out.push(check("network", "Network address", CheckStatus::Fail, "This machine has no LAN address, so nobody can reach it. Check the network cable or Wi-Fi."));
    } else {
        let list: Vec<String> = ips.iter().map(|ip| format!("{ip}:{port}")).collect();
        out.push(check(
            "network",
            "Network address",
            CheckStatus::Ok,
            format!("Clients can connect to {}", list.join(", ")),
        ));
    }

    // Firewall.
    out.push(firewall_check(port).await);

    // mDNS.
    let cfg = engine.config();
    if !cfg.mdns_enabled {
        out.push(check(
            "mdns",
            "Automatic discovery (mDNS)",
            CheckStatus::Warn,
            "Turned off in settings; clients must enter the address manually",
        ));
    } else if let Some(instance) = engine.mdns_instance() {
        out.push(check(
            "mdns",
            "Automatic discovery (mDNS)",
            CheckStatus::Ok,
            format!("Advertising as \"{instance}\""),
        ));
    } else {
        out.push(check(
            "mdns",
            "Automatic discovery (mDNS)",
            CheckStatus::Fail,
            engine
                .mdns_error()
                .unwrap_or_else(|| "Not advertising".into()),
        ));
    }

    // TLS.
    let tls = engine.tls();
    let days = (tls.not_after_ms - sl_proto::now_ms()) / 86_400_000;
    let status = if days > 60 {
        CheckStatus::Ok
    } else if days > 0 {
        CheckStatus::Warn
    } else {
        CheckStatus::Fail
    };
    out.push(check(
        "tls",
        "Server identity (TLS)",
        status,
        format!(
            "Fingerprint {} · valid for {days} more days",
            tls.fingerprint
        ),
    ));

    // Database.
    let db = engine.db().clone();
    let quick = db
        .read(|c| Ok(c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))?))
        .await;
    let wal = db.wal_size_bytes();
    match quick {
        Ok(s) if s == "ok" => {
            let status = if wal > 256 * 1024 * 1024 {
                CheckStatus::Warn
            } else {
                CheckStatus::Ok
            };
            out.push(check(
                "database",
                "Database",
                status,
                format!(
                    "Integrity ok · {} · WAL {}",
                    human_bytes(db.size_bytes()),
                    human_bytes(wal)
                ),
            ));
        }
        Ok(s) => out.push(check(
            "database",
            "Database",
            CheckStatus::Fail,
            format!("Integrity check reported: {s}. Restore a backup."),
        )),
        Err(e) => out.push(check(
            "database",
            "Database",
            CheckStatus::Fail,
            format!("Could not check: {e}"),
        )),
    }

    // Disk.
    let stats = engine.system_stats();
    let status = if stats.disk_total == 0 {
        CheckStatus::Unknown
    } else if stats.disk_free < 200 * 1024 * 1024 {
        CheckStatus::Fail
    } else if stats.disk_free < 1024 * 1024 * 1024 {
        CheckStatus::Warn
    } else {
        CheckStatus::Ok
    };
    out.push(check(
        "disk",
        "Disk space",
        status,
        format!(
            "{} free of {}",
            human_bytes(stats.disk_free),
            human_bytes(stats.disk_total)
        ),
    ));

    // Clock.
    let year = 1970 + sl_proto::now_ms() / 1000 / 31_557_600;
    if year < 2024 {
        out.push(check(
            "clock",
            "System clock",
            CheckStatus::Fail,
            "The clock looks wrong; invites and sessions may expire unexpectedly",
        ));
    } else {
        out.push(check(
            "clock",
            "System clock",
            CheckStatus::Ok,
            "Looks right",
        ));
    }

    // Standby copies.
    out.push(crate::standby::health(engine));

    // Backups.
    let backups = crate::admin::list_backup_files(&engine.paths().backups);
    let interval_ms = i64::from(cfg.backup_interval_hours.max(1)) * 3_600_000;
    match backups.first() {
        None => out.push(check(
            "backup",
            "Backups",
            CheckStatus::Unknown,
            "No backup yet (the first runs about an hour after start)",
        )),
        Some(b) => {
            let age = sl_proto::now_ms() - b.created_at;
            let status = if age > 2 * interval_ms {
                CheckStatus::Warn
            } else {
                CheckStatus::Ok
            };
            out.push(check(
                "backup",
                "Backups",
                status,
                format!("Last backup {} ago ({})", human_age(age), b.file_name),
            ));
        }
    }

    out
}

async fn firewall_check(port: u16) -> HealthCheck {
    if !cfg!(windows) {
        return check(
            "firewall",
            "Firewall",
            CheckStatus::Unknown,
            "Not checked on this operating system",
        );
    }
    let rule = format!("name={FIREWALL_RULE_TCP}");
    let res = tokio::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "show", "rule", &rule])
        .kill_on_drop(true)
        .output()
        .await;
    match res {
        Ok(o) if o.status.success() => check(
            "firewall",
            "Windows Firewall",
            CheckStatus::Ok,
            format!("Inbound rule \"{FIREWALL_RULE_TCP}\" allows port {port}"),
        ),
        Ok(_) => HealthCheck {
            id: "firewall".into(),
            name: "Windows Firewall".into(),
            status: CheckStatus::Warn,
            detail: format!(
                "No SpareLine firewall rule found. If staff can't connect, allow TCP {port} and mDNS (UDP 5353)."
            ),
            fix: Some(FixAction {
                id: "firewall".into(),
                label: "Add firewall rules".into(),
            }),
        },
        Err(e) => check(
            "firewall",
            "Windows Firewall",
            CheckStatus::Unknown,
            format!("Could not run netsh: {e}"),
        ),
    }
}

/// Run a fix. Returns a message for the admin.
pub async fn run_fix(engine: &Engine, id: &str) -> AppResult<String> {
    match id {
        "firewall" if cfg!(windows) => {
            let port = engine.config().port;
            let exe = std::env::current_exe()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let cmd = format!(
                "netsh advfirewall firewall delete rule name=\"{tcp}\" & \
                 netsh advfirewall firewall add rule name=\"{tcp}\" dir=in action=allow protocol=TCP localport={port} profile=private,domain & \
                 netsh advfirewall firewall delete rule name=\"{mdns}\" & \
                 netsh advfirewall firewall add rule name=\"{mdns}\" dir=in action=allow protocol=UDP localport=5353 program=\"{exe}\" profile=private,domain",
                tcp = FIREWALL_RULE_TCP,
                mdns = FIREWALL_RULE_MDNS,
            );
            // Elevate via UAC. The admin sees a Windows prompt.
            let ps = format!(
                "Start-Process -FilePath cmd.exe -ArgumentList '/c {}' -Verb RunAs -Wait -WindowStyle Hidden",
                cmd.replace('\'', "''")
            );
            let out = tokio::process::Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
                .output()
                .await?;
            engine.audit("health.fix", "firewall", "");
            if out.status.success() {
                Ok("Firewall rules added. Run the checks again to confirm.".into())
            } else {
                api_detail(
                    sl_proto::ErrorCode::Internal,
                    "The firewall change was cancelled or failed (it needs administrator approval).",
                )
            }
        }
        _ => api_detail(sl_proto::ErrorCode::NotFound, "no fix available"),
    }
}

/// Connect to ourselves the way a client would: LAN IP → TCP → TLS (pinned) → WS → Hello,
/// then check mDNS shows our advertisement.
pub async fn self_test(engine: &Engine) -> Vec<SelfTestStep> {
    let mut steps = Vec::new();
    let Some(local) = engine.local_addr() else {
        steps.push(SelfTestStep {
            name: "Server running".into(),
            status: CheckStatus::Fail,
            detail: "The server is stopped".into(),
            millis: 0,
        });
        return steps;
    };
    let ips = sl_net::lan_ipv4();
    let target = match ips.first() {
        Some(ip) => {
            steps.push(SelfTestStep {
                name: "LAN address".into(),
                status: CheckStatus::Ok,
                detail: ip.to_string(),
                millis: 0,
            });
            format!("{ip}:{}", local.port())
        }
        None => {
            steps.push(SelfTestStep {
                name: "LAN address".into(),
                status: CheckStatus::Warn,
                detail: "No LAN address; testing on 127.0.0.1 only".into(),
                millis: 0,
            });
            format!("127.0.0.1:{}", local.port())
        }
    };
    let verifier = sl_net::PinVerifier::new(sl_net::PinMode::Pinned(engine.tls().spki_sha256));
    match sl_net::dial(&target, verifier, Duration::from_secs(5)).await {
        Ok(mut d) => {
            for s in d.steps.drain(..) {
                steps.push(SelfTestStep {
                    name: s.name.into(),
                    status: CheckStatus::Ok,
                    detail: s.detail,
                    millis: s.millis,
                });
            }
            let t = Instant::now();
            let hello = tokio::time::timeout(Duration::from_secs(3), d.ws.next()).await;
            let ok = matches!(
                hello,
                Ok(Some(Ok(sl_net::tokio_tungstenite::tungstenite::Message::Text(ref t))))
                    if matches!(serde_json::from_str::<ServerFrame>(t.as_str()), Ok(ServerFrame::Hello { .. }))
            );
            steps.push(SelfTestStep {
                name: "SpareLine handshake".into(),
                status: if ok {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Fail
                },
                detail: if ok {
                    "Server greeted the test client".into()
                } else {
                    "No greeting received".into()
                },
                millis: t.elapsed().as_millis() as u64,
            });
            let _ = d.ws.close(None).await;
        }
        Err((err, partial)) => {
            for s in partial {
                steps.push(SelfTestStep {
                    name: s.name.into(),
                    status: if s.ok {
                        CheckStatus::Ok
                    } else {
                        CheckStatus::Fail
                    },
                    detail: s.detail,
                    millis: s.millis,
                });
            }
            steps.push(SelfTestStep {
                name: "Result".into(),
                status: CheckStatus::Fail,
                detail: err.to_string(),
                millis: 0,
            });
        }
    }

    if engine.config().mdns_enabled {
        let t = Instant::now();
        let id = engine.config().server_id.to_string();
        let found =
            tokio::task::spawn_blocking(|| crate::mdns::browse_once(Duration::from_secs(3))).await;
        let (status, detail) = match found {
            Ok(Ok(list))
                if list
                    .iter()
                    .any(|(_, _, sid)| sid.as_deref() == Some(id.as_str())) =>
            {
                (
                    CheckStatus::Ok,
                    format!(
                        "Visible on the LAN ({} SpareLine server(s) found)",
                        list.len()
                    ),
                )
            }
            Ok(Ok(list)) => (
                CheckStatus::Warn,
                format!(
                    "Our advertisement was not seen ({} other server(s) found). Multicast may be blocked on this network.",
                    list.len()
                ),
            ),
            Ok(Err(e)) => (CheckStatus::Fail, format!("mDNS error: {e}")),
            Err(e) => (CheckStatus::Fail, format!("mDNS error: {e}")),
        };
        steps.push(SelfTestStep {
            name: "Automatic discovery (mDNS)".into(),
            status,
            detail,
            millis: t.elapsed().as_millis() as u64,
        });
    }
    steps
}

pub fn collect_system(
    sys: &mut sysinfo::System,
    data_dir: &std::path::Path,
    db_size: u64,
) -> SystemStats {
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    let pid = sysinfo::get_current_pid().ok();
    if let Some(pid) = pid {
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    }
    let process_mem = pid
        .and_then(|p| sys.process(p))
        .map(|p| p.memory())
        .unwrap_or(0);
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut best: Option<(usize, u64, u64)> = None;
    let dir = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let dir_s = dir.to_string_lossy().to_lowercase().replace("\\\\?\\", "");
    for d in disks.list() {
        let mp = d.mount_point().to_string_lossy().to_lowercase();
        if dir_s.starts_with(&mp) && best.is_none_or(|(len, _, _)| mp.len() > len) {
            best = Some((mp.len(), d.available_space(), d.total_space()));
        }
    }
    let (disk_free, disk_total) = best.map(|(_, a, t)| (a, t)).unwrap_or((0, 0));
    SystemStats {
        cpu_percent: sys.global_cpu_usage(),
        mem_used: sys.used_memory(),
        mem_total: sys.total_memory(),
        process_mem,
        disk_free,
        disk_total,
        db_size,
    }
}

/// This machine's LAN IPv4 addresses as strings.
pub fn lan_addresses() -> Vec<String> {
    sl_net::lan_ipv4().iter().map(|ip| ip.to_string()).collect()
}

pub fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

pub fn human_age(ms: i64) -> String {
    let s = ms / 1000;
    if s < 90 {
        format!("{s} s")
    } else if s < 5400 {
        format!("{} min", s / 60)
    } else if s < 172_800 {
        format!("{} h", s / 3600)
    } else {
        format!("{} days", s / 86_400)
    }
}
