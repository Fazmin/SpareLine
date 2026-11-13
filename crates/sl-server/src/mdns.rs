//! LAN discovery: advertise `_spareline._tcp.local.` with mDNS / DNS-SD.

use mdns_sd::{ServiceDaemon, ServiceInfo};

pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
    pub instance: String,
}

impl Advertiser {
    pub fn start(
        server_id: uuid::Uuid,
        server_name: &str,
        port: u16,
        fingerprint_hint: &str,
    ) -> anyhow::Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let short = &server_id.simple().to_string()[..6];
        // Instance names are free text (up to 63 bytes).
        let mut instance: String = server_name
            .chars()
            .filter(|c| !c.is_control() && *c != '.')
            .take(48)
            .collect();
        if instance.trim().is_empty() {
            instance = "SpareLine".into();
        }
        let instance = format!("{} ({short})", instance.trim());
        let host = sanitize_host(&sl_net::hostname());
        let host_name = format!("{host}.local.");
        let id = server_id.to_string();
        let proto = sl_proto::PROTOCOL_VERSION.to_string();
        let props = [
            ("v", proto.as_str()),
            ("id", id.as_str()),
            ("name", server_name),
            ("fp", fingerprint_hint),
        ];
        let info = ServiceInfo::new(
            sl_proto::MDNS_SERVICE_TYPE,
            &instance,
            &host_name,
            "",
            port,
            &props[..],
        )?
        .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon.register(info)?;
        tracing::info!(target: "sl_server::mdns", %instance, port, "advertising on the LAN");
        Ok(Self {
            daemon,
            fullname,
            instance,
        })
    }

    pub fn stop(self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
        tracing::info!(target: "sl_server::mdns", "stopped advertising");
    }
}

fn sanitize_host(h: &str) -> String {
    let s: String = h
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "spareline-server".into()
    } else {
        s
    }
}

/// `(instance, host:port list, server id from TXT)`.
pub type Found = (String, Vec<String>, Option<String>);

/// Browse for SpareLine servers for up to `wait`.
/// Used by the server self-test to confirm its own advertisement is visible.
pub fn browse_once(wait: std::time::Duration) -> anyhow::Result<Vec<Found>> {
    let daemon = ServiceDaemon::new()?;
    let rx = daemon.browse(sl_proto::MDNS_SERVICE_TYPE)?;
    let deadline = std::time::Instant::now() + wait;
    let mut out = Vec::new();
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                let addrs = info
                    .get_addresses_v4()
                    .iter()
                    .map(|a| format!("{a}:{}", info.get_port()))
                    .collect();
                out.push((
                    info.get_fullname().to_string(),
                    addrs,
                    info.get_property_val_str("id").map(str::to_string),
                ));
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.stop_browse(sl_proto::MDNS_SERVICE_TYPE);
    let _ = daemon.shutdown();
    Ok(out)
}
