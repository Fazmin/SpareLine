//! Native networking shared by the SpareLine server (self-test) and client.
//!
//! Trust model: SpareLine servers use a self-signed certificate. Clients do not check
//! names or CAs; they pin the SHA-256 hash of the server's SubjectPublicKeyInfo (SPKI).
//! The first pin is established by an invite-code proof (see `sl_proto::crypto`).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::{DigitallySignedStruct, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::WebSocketStream;

pub use tokio_tungstenite;

/// The ring crypto provider (no C toolchain beyond what `ring` needs).
pub fn crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

#[derive(Debug, thiserror::Error)]
pub enum CertError {
    #[error("certificate could not be parsed: {0}")]
    Parse(String),
}

/// SHA-256 of the certificate's SubjectPublicKeyInfo DER.
pub fn spki_sha256(cert_der: &[u8]) -> Result<[u8; 32], CertError> {
    let (_, cert) = x509_parser::parse_x509_certificate(cert_der)
        .map_err(|e| CertError::Parse(e.to_string()))?;
    Ok(sl_proto::crypto::sha256(cert.public_key().raw))
}

/// Certificate expiry as unix milliseconds.
pub fn cert_not_after_ms(cert_der: &[u8]) -> Result<i64, CertError> {
    let (_, cert) = x509_parser::parse_x509_certificate(cert_der)
        .map_err(|e| CertError::Parse(e.to_string()))?;
    Ok(cert.validity().not_after.timestamp() * 1000)
}

/// How the verifier treats the presented certificate.
#[derive(Debug, Clone, Copy)]
pub enum PinMode {
    /// Accept any certificate and record its SPKI hash (used only before an invite proof).
    Capture,
    /// Accept only this SPKI hash.
    Pinned([u8; 32]),
}

/// rustls verifier implementing SPKI pinning.
#[derive(Debug)]
pub struct PinVerifier {
    mode: PinMode,
    seen: Mutex<Option<[u8; 32]>>,
    provider: Arc<CryptoProvider>,
}

impl PinVerifier {
    pub fn new(mode: PinMode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            seen: Mutex::new(None),
            provider: crypto_provider(),
        })
    }
    /// SPKI hash of the last certificate presented (even if rejected).
    pub fn seen(&self) -> Option<[u8; 32]> {
        *self.seen.lock()
    }
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let hash = spki_sha256(end_entity.as_ref()).map_err(|_| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
        })?;
        *self.seen.lock() = Some(hash);
        match self.mode {
            PinMode::Capture => Ok(ServerCertVerified::assertion()),
            PinMode::Pinned(expected) => {
                if sl_proto::crypto::ct_eq(&expected, &hash) {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(rustls::Error::InvalidCertificate(
                        rustls::CertificateError::ApplicationVerificationFailure,
                    ))
                }
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub type WsStream = WebSocketStream<TlsStream<TcpStream>>;

/// One step of a connection attempt, for diagnostics.
#[derive(Debug, Clone)]
pub struct Step {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
    pub millis: u64,
}

/// Why a connection attempt failed, in categories people can act on.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ConnectError {
    #[error("could not resolve address {0}")]
    Resolve(String),
    #[error("connection timed out (server off, wrong address, or firewall)")]
    TcpTimeout,
    #[error("connection refused (SpareLine Server not running on that port)")]
    TcpRefused,
    #[error("network error: {0}")]
    Tcp(String),
    #[error("server identity does not match the pinned fingerprint")]
    PinMismatch,
    #[error("secure connection failed: {0}")]
    Tls(String),
    #[error("WebSocket handshake failed: {0}")]
    WebSocket(String),
}

impl ConnectError {
    /// Short machine-friendly category used in client health reports.
    pub fn category(&self) -> &'static str {
        match self {
            Self::Resolve(_) => "resolve",
            Self::TcpTimeout => "tcp_timeout",
            Self::TcpRefused => "tcp_refused",
            Self::Tcp(_) => "tcp_error",
            Self::PinMismatch => "pin_mismatch",
            Self::Tls(_) => "tls_handshake",
            Self::WebSocket(_) => "ws_handshake",
        }
    }
}

/// Result of a successful dial.
pub struct Dialed {
    pub ws: WsStream,
    pub peer: SocketAddr,
    pub steps: Vec<Step>,
    pub spki_sha256: [u8; 32],
}

/// Dial `host:port`, do TLS with the given pin verifier, then the WebSocket upgrade.
/// On failure returns the error plus the steps completed so far.
pub async fn dial(
    address: &str,
    verifier: Arc<PinVerifier>,
    timeout: Duration,
) -> Result<Dialed, (ConnectError, Vec<Step>)> {
    let mut steps = Vec::new();
    let t0 = Instant::now();

    // Resolve.
    let addrs: Vec<SocketAddr> =
        match tokio::time::timeout(timeout, tokio::net::lookup_host(address)).await {
            Ok(Ok(a)) => a.collect(),
            _ => {
                steps.push(step("Resolve address", false, address.to_string(), t0));
                return Err((ConnectError::Resolve(address.to_string()), steps));
            }
        };
    // Prefer IPv4 (LAN servers rarely have routable v6 and mDNS gives v4).
    let mut addrs = addrs;
    addrs.sort_by_key(|a| if a.is_ipv4() { 0 } else { 1 });
    let Some(peer) = addrs.first().copied() else {
        steps.push(step("Resolve address", false, "no addresses".into(), t0));
        return Err((ConnectError::Resolve(address.to_string()), steps));
    };
    steps.push(step("Resolve address", true, peer.to_string(), t0));

    // TCP.
    let t1 = Instant::now();
    let tcp = match tokio::time::timeout(timeout, TcpStream::connect(peer)).await {
        Err(_) => {
            steps.push(step("Reach server (TCP)", false, "timed out".into(), t1));
            return Err((ConnectError::TcpTimeout, steps));
        }
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            steps.push(step("Reach server (TCP)", false, "refused".into(), t1));
            return Err((ConnectError::TcpRefused, steps));
        }
        Ok(Err(e)) => {
            steps.push(step("Reach server (TCP)", false, e.to_string(), t1));
            return Err((ConnectError::Tcp(e.to_string()), steps));
        }
        Ok(Ok(s)) => s,
    };
    let _ = tcp.set_nodelay(true);
    steps.push(step("Reach server (TCP)", true, peer.to_string(), t1));

    // TLS with pinning.
    let t2 = Instant::now();
    let config = match rustls::ClientConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
    {
        Ok(b) => b
            .dangerous()
            .with_custom_certificate_verifier(verifier.clone())
            .with_no_client_auth(),
        Err(e) => {
            steps.push(step("Secure connection (TLS)", false, e.to_string(), t2));
            return Err((ConnectError::Tls(e.to_string()), steps));
        }
    };
    let connector = TlsConnector::from(Arc::new(config));
    // Names are not verified (pinning instead); any syntactically valid name works.
    let server_name = ServerName::IpAddress(peer.ip().into());
    let tls = match tokio::time::timeout(timeout, connector.connect(server_name, tcp)).await {
        Err(_) => {
            steps.push(step(
                "Secure connection (TLS)",
                false,
                "timed out".into(),
                t2,
            ));
            return Err((ConnectError::Tls("timed out".into()), steps));
        }
        Ok(Err(e)) => {
            let mismatch = e.to_string().contains("ApplicationVerificationFailure");
            if mismatch {
                steps.push(step(
                    "Verify server identity",
                    false,
                    "fingerprint mismatch".into(),
                    t2,
                ));
                return Err((ConnectError::PinMismatch, steps));
            }
            steps.push(step("Secure connection (TLS)", false, e.to_string(), t2));
            return Err((ConnectError::Tls(e.to_string()), steps));
        }
        Ok(Ok(s)) => s,
    };
    let spki = verifier.seen().unwrap_or([0u8; 32]);
    steps.push(step(
        "Secure connection (TLS)",
        true,
        sl_proto::crypto::fingerprint_display(&spki),
        t2,
    ));

    // WebSocket upgrade.
    let t3 = Instant::now();
    let url = format!("wss://{}{}", peer, sl_proto::WS_PATH);
    let ws = match tokio::time::timeout(timeout, tokio_tungstenite::client_async(url, tls)).await {
        Err(_) => {
            steps.push(step(
                "Open chat channel (WebSocket)",
                false,
                "timed out".into(),
                t3,
            ));
            return Err((ConnectError::WebSocket("timed out".into()), steps));
        }
        Ok(Err(e)) => {
            steps.push(step(
                "Open chat channel (WebSocket)",
                false,
                e.to_string(),
                t3,
            ));
            return Err((ConnectError::WebSocket(e.to_string()), steps));
        }
        Ok(Ok((ws, _resp))) => ws,
    };
    steps.push(step(
        "Open chat channel (WebSocket)",
        true,
        String::new(),
        t3,
    ));
    Ok(Dialed {
        ws,
        peer,
        steps,
        spki_sha256: spki,
    })
}

fn step(name: &'static str, ok: bool, detail: String, since: Instant) -> Step {
    Step {
        name,
        ok,
        detail,
        millis: since.elapsed().as_millis() as u64,
    }
}

/// Adapter names that are almost always virtual (WSL, Hyper-V, VMs, containers, VPN taps).
fn is_virtual_adapter(name: &str) -> bool {
    let n = name.to_lowercase();
    [
        "vethernet",
        "wsl",
        "hyper-v",
        "virtualbox",
        "vmware",
        "vmnet",
        "docker",
        "br-",
        "veth",
        "virbr",
        "loopback",
        "npcap",
        "tap-",
        "utun",
        "awdl",
        "llw",
        "bridge",
    ]
    .iter()
    .any(|v| n.contains(v))
}

/// LAN IPv4 addresses of this machine: physical adapters first, private ranges first.
/// Virtual adapters are only used if there is nothing else.
pub fn lan_ipv4() -> Vec<Ipv4Addr> {
    let all: Vec<(bool, Ipv4Addr)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) if !v4.is_link_local() && !v4.is_unspecified() => {
                Some((is_virtual_adapter(&i.name), v4))
            }
            _ => None,
        })
        .collect();
    let physical: Vec<Ipv4Addr> = all.iter().filter(|(v, _)| !v).map(|(_, ip)| *ip).collect();
    let mut out = if physical.is_empty() {
        all.into_iter().map(|(_, ip)| ip).collect()
    } else {
        physical
    };
    out.sort_by_key(|ip| if ip.is_private() { 0 } else { 1 });
    out.dedup();
    out
}

/// Best-effort machine host name.
pub fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "spareline-server".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_adapters_are_recognised() {
        assert!(is_virtual_adapter("vEthernet (WSL (Hyper-V firewall))"));
        assert!(is_virtual_adapter("docker0"));
        assert!(!is_virtual_adapter("Ethernet"));
        assert!(!is_virtual_adapter("Wi-Fi"));
        assert!(!is_virtual_adapter("en0"));
    }

    #[test]
    fn hostname_is_not_empty() {
        assert!(!hostname().is_empty());
    }
}
