//! Find SpareLine servers on the LAN with mDNS / DNS-SD.

use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent};
use sl_proto::client_ipc::{DiscoveredServer, DiscoverySource};

/// Browse for up to `wait` (returns early once results stop changing for a moment).
pub async fn browse(wait: Duration) -> Vec<DiscoveredServer> {
    tokio::task::spawn_blocking(move || browse_blocking(wait))
        .await
        .unwrap_or_default()
}

fn browse_blocking(wait: Duration) -> Vec<DiscoveredServer> {
    let Ok(daemon) = ServiceDaemon::new() else {
        return Vec::new();
    };
    let Ok(rx) = daemon.browse(sl_proto::MDNS_SERVICE_TYPE) else {
        return Vec::new();
    };
    let deadline = Instant::now() + wait;
    let mut found: Vec<DiscoveredServer> = Vec::new();
    let mut last_new = Instant::now();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        // Once we have something and nothing new arrived for 700 ms, stop early.
        if !found.is_empty() && last_new.elapsed() > Duration::from_millis(700) {
            break;
        }
        match rx.recv_timeout(left.min(Duration::from_millis(200))) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let port = info.get_port();
                let mut addrs: Vec<String> = info
                    .get_addresses_v4()
                    .iter()
                    .map(|a| format!("{a}:{port}"))
                    .collect();
                addrs.sort();
                let server_id = info.get_property_val_str("id").and_then(|s| s.parse().ok());
                let name = info
                    .get_property_val_str("name")
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        info.get_fullname()
                            .split("._spareline")
                            .next()
                            .unwrap_or("SpareLine")
                            .to_string()
                    });
                let fp = info.get_property_val_str("fp").map(str::to_string);
                if let Some(existing) = found
                    .iter_mut()
                    .find(|s| s.server_id.is_some() && s.server_id == server_id)
                {
                    for a in addrs {
                        if !existing.addresses.contains(&a) {
                            existing.addresses.push(a);
                        }
                    }
                } else if !addrs.is_empty() {
                    found.push(DiscoveredServer {
                        name,
                        server_id,
                        addresses: addrs,
                        fingerprint_hint: fp,
                        source: DiscoverySource::Mdns,
                    });
                    last_new = Instant::now();
                }
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    let _ = daemon.stop_browse(sl_proto::MDNS_SERVICE_TYPE);
    let _ = daemon.shutdown();
    found
}
