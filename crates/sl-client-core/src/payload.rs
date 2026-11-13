//! Application payloads carried inside MLS application messages. The server never sees
//! these: they exist only on member devices.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_BODY_CHARS: usize = 8000;
pub const QUICK_REACTIONS: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🎉"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Payload {
    Text {
        id: Uuid,
        body: String,
        #[serde(default)]
        reply_to: Option<Uuid>,
    },
    Edit {
        target: Uuid,
        body: String,
    },
    Delete {
        target: Uuid,
    },
    Reaction {
        target: Uuid,
        emoji: String,
        add: bool,
    },
    ChatMeta {
        title: String,
    },
}

#[derive(Serialize, Deserialize)]
struct Wrapper {
    v: u8,
    p: Payload,
}

pub fn encode(p: &Payload) -> Vec<u8> {
    serde_json::to_vec(&Wrapper { v: 1, p: p.clone() }).unwrap_or_default()
}

pub fn decode(bytes: &[u8]) -> Option<Payload> {
    let w: Wrapper = serde_json::from_slice(bytes).ok()?;
    (w.v == 1).then_some(w.p)
}

/// Emoji allowed in reactions: short, no control characters.
pub fn valid_emoji(e: &str) -> bool {
    let n = e.chars().count();
    (1..=8).contains(&n)
        && !e
            .chars()
            .any(|c| c.is_control() || c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let p = Payload::Text {
            id: Uuid::now_v7(),
            body: "hi".into(),
            reply_to: None,
        };
        assert_eq!(decode(&encode(&p)), Some(p));
        assert!(decode(b"{\"v\":9,\"p\":{\"t\":\"delete\",\"target\":\"00000000-0000-0000-0000-000000000000\"}}").is_none());
        assert!(valid_emoji("👍"));
        assert!(!valid_emoji("lol"));
    }
}
