//! Base64 helpers for serde, plus the [`Bytes`] wrapper used for every opaque blob.

use data_encoding::BASE64;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&BASE64.encode(bytes))
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let s = String::deserialize(d)?;
    BASE64
        .decode(s.as_bytes())
        .map_err(serde::de::Error::custom)
}

/// Opaque binary payload (MLS messages, key packages, nonces...).
///
/// `Debug` and `Display` deliberately print only the length so that payloads can never
/// leak into logs by accident.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Bytes(#[serde(with = "crate::b64")] pub Vec<u8>);

impl Bytes {
    pub fn new(v: Vec<u8>) -> Self {
        Self(v)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for Bytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{} bytes>", self.0.len())
    }
}

impl std::fmt::Display for Bytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{} bytes>", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_never_debug_print_content() {
        let b = Bytes(b"secret message".to_vec());
        assert_eq!(format!("{b:?}"), "<14 bytes>");
        assert_eq!(format!("{b}"), "<14 bytes>");
    }

    #[test]
    fn bytes_roundtrip_json() {
        let b = Bytes(vec![0, 1, 2, 250]);
        let j = serde_json::to_string(&b).expect("ser");
        assert_eq!(j, "\"AAEC+g==\"");
        let back: Bytes = serde_json::from_str(&j).expect("de");
        assert_eq!(back, b);
    }
}
