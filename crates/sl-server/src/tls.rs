//! Self-signed TLS identity, generated on first launch and pinned by clients.

use std::sync::Arc;

use rustls::ServerConfig;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::config::Paths;

#[derive(Clone)]
pub struct TlsIdentity {
    pub cert_der: Vec<u8>,
    key_der: Vec<u8>,
    pub spki_sha256: [u8; 32],
    /// Human-readable fingerprint (8 groups of 4 hex digits).
    pub fingerprint: String,
    /// Short hint advertised over mDNS (first 8 hex digits).
    pub fingerprint_hint: String,
    pub not_after_ms: i64,
}

impl std::fmt::Debug for TlsIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsIdentity")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl TlsIdentity {
    /// Load the identity from disk, or create one (valid ~10 years) if missing.
    pub fn load_or_create(paths: &Paths) -> anyhow::Result<Self> {
        if paths.cert.exists() && paths.key.exists() {
            let cert_der = std::fs::read(&paths.cert)?;
            let key_der = std::fs::read(&paths.key)?;
            return Self::from_der(cert_der, key_der);
        }
        let (cert_der, key_der) = generate()?;
        std::fs::write(&paths.cert, &cert_der)?;
        write_private(&paths.key, &key_der)?;
        // PEM copy of the public certificate, for admins who want to inspect it.
        let pem = pem_encode("CERTIFICATE", &cert_der);
        let _ = std::fs::write(paths.tls_dir.join("server.crt.pem"), pem);
        Self::from_der(cert_der, key_der)
    }

    /// Adopt an identity from a standby copy, so apps that pinned the old server trust
    /// this one.
    pub fn install(paths: &Paths, cert_der: Vec<u8>, key_der: Vec<u8>) -> anyhow::Result<Self> {
        let id = Self::from_der(cert_der, key_der)?;
        id.server_config()?;
        std::fs::create_dir_all(&paths.tls_dir)?;
        std::fs::write(&paths.cert, &id.cert_der)?;
        write_private(&paths.key, &id.key_der)?;
        let pem = pem_encode("CERTIFICATE", &id.cert_der);
        let _ = std::fs::write(paths.tls_dir.join("server.crt.pem"), pem);
        Ok(id)
    }

    /// The private key, for standby copies only.
    pub(crate) fn key_der(&self) -> &[u8] {
        &self.key_der
    }

    /// Replace the identity with a fresh one. All clients will need setup codes.
    pub fn regenerate(paths: &Paths) -> anyhow::Result<Self> {
        let _ = std::fs::remove_file(&paths.cert);
        let _ = std::fs::remove_file(&paths.key);
        Self::load_or_create(paths)
    }

    fn from_der(cert_der: Vec<u8>, key_der: Vec<u8>) -> anyhow::Result<Self> {
        let spki_sha256 = sl_net::spki_sha256(&cert_der)?;
        let not_after_ms = sl_net::cert_not_after_ms(&cert_der)?;
        let fingerprint = sl_proto::crypto::fingerprint_display(&spki_sha256);
        let fingerprint_hint = data_encoding::HEXUPPER.encode(&spki_sha256[..4]);
        Ok(Self {
            cert_der,
            key_der,
            spki_sha256,
            fingerprint,
            fingerprint_hint,
            not_after_ms,
        })
    }

    pub fn server_config(&self) -> anyhow::Result<Arc<ServerConfig>> {
        let cert = CertificateDer::from(self.cert_der.clone());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der.clone()));
        let mut cfg = ServerConfig::builder_with_provider(sl_net::crypto_provider())
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)?;
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(cfg))
    }
}

fn generate() -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let host = sl_net::hostname();
    let mut sans = vec!["spareline.local".to_string(), "localhost".to_string()];
    let h = host.to_ascii_lowercase();
    if h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        sans.push(h.clone());
        sans.push(format!("{h}.local"));
    }
    for ip in sl_net::lan_ipv4() {
        sans.push(ip.to_string());
    }
    let mut params = rcgen::CertificateParams::new(sans)?;
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(
        rcgen::DnType::CommonName,
        format!("SpareLine Server ({host})"),
    );
    dn.push(rcgen::DnType::OrganizationName, "SpareLine");
    params.distinguished_name = dn;
    let year = 1970 + (sl_proto::now_ms() / 1000 / 31_557_600) as i32;
    params.not_before = rcgen::date_time_ymd(year - 1, 1, 1);
    params.not_after = rcgen::date_time_ymd(year + 10, 1, 1);
    let key = rcgen::KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    Ok((cert.der().to_vec(), key.serialize_der()))
}

pub(crate) fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    // On Windows the file inherits the per-user %LOCALAPPDATA% ACL (owner + SYSTEM + Administrators).
    Ok(())
}

fn pem_encode(label: &str, der: &[u8]) -> String {
    let b64 = data_encoding::BASE64.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_and_reloads_same_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        paths.ensure().expect("dirs");
        let a = TlsIdentity::load_or_create(&paths).expect("create");
        let b = TlsIdentity::load_or_create(&paths).expect("reload");
        assert_eq!(a.spki_sha256, b.spki_sha256);
        assert!(a.not_after_ms > sl_proto::now_ms() + 365 * 24 * 3600 * 1000);
        assert!(a.server_config().is_ok());
    }
}
