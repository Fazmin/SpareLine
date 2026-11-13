//! Invite-code derivations and server-identity proofs (native engines only).
//!
//! An invite code is a shared secret between the admin's server and the new user.
//! From it both sides derive:
//! - `invite_id`: a public lookup key the client can send before trusting the server;
//! - `proof_key`: used by the server to prove, over the TLS connection the client is
//!   using, that it really holds the invite. The proof binds the server's certificate
//!   (SPKI hash), so a rogue server or spoofed mDNS answer cannot produce it.

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const INVITE_SALT: &[u8] = b"spareline/invite/v1";
const PROOF_LABEL: &[u8] = b"spareline/prove/v1";

/// Values derived from a canonical invite code.
#[derive(Clone)]
pub struct InviteKeys {
    /// Hex, 32 chars. Safe to send in the clear.
    pub invite_id: String,
    /// Secret. Stored server-side; never sent.
    pub proof_key: [u8; 32],
}

impl std::fmt::Debug for InviteKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InviteKeys")
            .field("invite_id", &self.invite_id)
            .finish_non_exhaustive()
    }
}

/// Derive the invite id and proof key from a canonical code (see [`crate::code::normalize`]).
pub fn derive_invite(canonical_code: &str) -> InviteKeys {
    let hk = Hkdf::<Sha256>::new(Some(INVITE_SALT), canonical_code.as_bytes());
    let mut id = [0u8; 16];
    let mut proof_key = [0u8; 32];
    // Lengths are fixed and far below the HKDF limit; expand cannot fail.
    hk.expand(b"id", &mut id)
        .expect("HKDF output length is valid");
    hk.expand(b"proof", &mut proof_key)
        .expect("HKDF output length is valid");
    InviteKeys {
        invite_id: hex(&id),
        proof_key,
    }
}

/// Server side: compute the proof for a client's nonce and our certificate's SPKI hash.
pub fn compute_proof(proof_key: &[u8; 32], nonce: &[u8], spki_sha256: &[u8; 32]) -> [u8; 32] {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(proof_key).expect("HMAC accepts any key length");
    mac.update(PROOF_LABEL);
    mac.update(&(nonce.len() as u32).to_be_bytes());
    mac.update(nonce);
    mac.update(spki_sha256);
    mac.finalize().into_bytes().into()
}

/// Client side: verify a proof in constant time.
pub fn verify_proof(
    proof_key: &[u8; 32],
    nonce: &[u8],
    spki_sha256: &[u8; 32],
    proof: &[u8],
) -> bool {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(proof_key).expect("HMAC accepts any key length");
    mac.update(PROOF_LABEL);
    mac.update(&(nonce.len() as u32).to_be_bytes());
    mac.update(nonce);
    mac.update(spki_sha256);
    mac.verify_slice(proof).is_ok()
}

/// Constant-time equality for secrets.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Generate a fresh canonical invite code (80 bits).
pub fn generate_code() -> String {
    let mut rng = rand::rngs::OsRng;
    let mut out = String::with_capacity(crate::code::CODE_LEN);
    for _ in 0..crate::code::CODE_LEN {
        let idx = (rng.next_u32() % 32) as usize;
        out.push(crate::code::ALPHABET[idx] as char);
    }
    out
}

/// Generate a random session token (256 bits, base64url without padding).
pub fn generate_token() -> String {
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    data_encoding::BASE64URL_NOPAD.encode(&b)
}

/// Random bytes helper.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b
}

/// SHA-256 → lowercase hex. Used to store session tokens server-side.
pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// SHA-256 raw.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub fn hex(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(bytes)
}

/// Human-readable fingerprint: first 16 bytes as 8 groups of 4 upper-case hex digits.
/// Example: `4F2A 91C0 7B3E 0D55 A1C9 33F0 9E12 6B7D`.
pub fn fingerprint_display(spki_sha256: &[u8]) -> String {
    let hexs = data_encoding::HEXUPPER.encode(&spki_sha256[..spki_sha256.len().min(16)]);
    hexs.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_roundtrip_and_binding() {
        let code = crate::code::normalize("SPL-ABCD-EFGH-JKMN-PQRS").expect("valid");
        let k = derive_invite(&code);
        assert_eq!(k.invite_id.len(), 32);
        let nonce = random_bytes::<32>();
        let spki = sha256(b"server-cert-spki");
        let proof = compute_proof(&k.proof_key, &nonce, &spki);
        assert!(verify_proof(&k.proof_key, &nonce, &spki, &proof));
        // A different certificate (rogue server) must fail.
        let rogue = sha256(b"rogue-cert-spki");
        assert!(!verify_proof(&k.proof_key, &nonce, &rogue, &proof));
        // A different code must fail.
        let other =
            derive_invite(&crate::code::normalize("SPL-0000-0000-0000-0000").expect("valid"));
        assert!(!verify_proof(&other.proof_key, &nonce, &spki, &proof));
    }

    #[test]
    fn generated_codes_are_valid() {
        for _ in 0..100 {
            let c = generate_code();
            assert_eq!(crate::code::normalize(&c).as_deref(), Some(c.as_str()));
        }
    }

    #[test]
    fn fingerprint_format() {
        let fp = fingerprint_display(&[0xAB; 32]);
        assert_eq!(fp, "ABAB ABAB ABAB ABAB ABAB ABAB ABAB ABAB");
    }
}
