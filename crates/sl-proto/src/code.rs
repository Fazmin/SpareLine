//! Invite / setup code format: `SPL-XXXX-XXXX-XXXX-XXXX`.
//!
//! 16 Crockford base32 symbols (80 bits). Parsing is forgiving: case-insensitive,
//! ignores spaces/dashes and the `SPL` prefix, and maps the commonly confused
//! letters `O`→`0`, `I`/`L`→`1`.

/// Crockford base32 alphabet (no I, L, O, U).
pub const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Number of symbols in a code (excluding prefix and separators).
pub const CODE_LEN: usize = 16;
pub const PREFIX: &str = "SPL";

/// Normalize user input into the canonical 16-symbol form, or `None` if invalid.
pub fn normalize(input: &str) -> Option<String> {
    let mut s: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if s.len() == CODE_LEN + PREFIX.len() && s.starts_with(PREFIX) {
        s.drain(..PREFIX.len());
    }
    if s.len() != CODE_LEN {
        return None;
    }
    let mut out = String::with_capacity(CODE_LEN);
    for c in s.chars() {
        let c = match c {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        };
        if !ALPHABET.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
    }
    Some(out)
}

/// Format a canonical code for display: `SPL-ABCD-EFGH-JKMN-PQRS`.
pub fn display(canonical: &str) -> String {
    let mut out = String::from(PREFIX);
    for (i, c) in canonical.chars().enumerate() {
        if i % 4 == 0 {
            out.push('-');
        }
        out.push(c);
    }
    out
}

/// Build a `spareline://join` deep link carrying the code and optional host hints.
pub fn join_link(canonical: &str, hosts: &[String], port: u16) -> String {
    let mut link = format!("spareline://join?code={}", display(canonical));
    for h in hosts {
        link.push_str("&host=");
        link.push_str(h);
        link.push(':');
        link.push_str(&port.to_string());
    }
    link
}

/// Parsed contents of a pasted invite message or `spareline://join` link.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedInvite {
    pub code: Option<String>,
    /// `host:port` hints.
    pub hosts: Vec<String>,
}

/// Extract a code and host hints from free text (an invite message, a link, or a bare code).
pub fn parse_invite_text(text: &str) -> ParsedInvite {
    let mut parsed = ParsedInvite::default();
    // Link form first.
    if let Some(idx) = text.find("spareline://join?") {
        let rest = &text[idx + "spareline://join?".len()..];
        let rest = rest.split_whitespace().next().unwrap_or("");
        for pair in rest.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                match k {
                    "code" => parsed.code = normalize(v),
                    "host" if !v.is_empty() => parsed.hosts.push(v.to_string()),
                    _ => {}
                }
            }
        }
    }
    if parsed.code.is_none() {
        // Look for a token that normalizes to a valid code.
        for token in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
            if let Some(c) =
                normalize(token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'))
            {
                parsed.code = Some(c);
                break;
            }
        }
    }
    if parsed.hosts.is_empty() {
        // "Server: 192.168.1.10:7443" style lines.
        for token in text.split_whitespace() {
            let t = token.trim_matches(|c: char| c == ',' || c == ';' || c == '(' || c == ')');
            if let Some((h, p)) = t.rsplit_once(':')
                && p.parse::<u16>().is_ok()
                && !h.is_empty()
                && h.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                && !h.starts_with("spareline")
            {
                parsed.hosts.push(t.to_string());
            }
        }
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_displays() {
        let canon = normalize("spl-abcd-efgh-jkmn-pqrs").expect("valid");
        assert_eq!(canon, "ABCDEFGHJKMNPQRS");
        assert_eq!(display(&canon), "SPL-ABCD-EFGH-JKMN-PQRS");
        assert_eq!(
            normalize("ABCD EFGH JKMN PQRS").as_deref(),
            Some("ABCDEFGHJKMNPQRS")
        );
        // Confusable letters map to digits.
        assert_eq!(
            normalize("OOOO-IIII-LLLL-0000").as_deref(),
            Some("0000111111110000")
        );
        // U is not in the alphabet.
        assert!(normalize("UUUU-UUUU-UUUU-UUUU").is_none());
        assert!(normalize("too-short").is_none());
    }

    #[test]
    fn parses_invite_messages() {
        let msg =
            "Join SpareLine: spareline://join?code=SPL-ABCD-EFGH-JKMN-PQRS&host=192.168.1.10:7443";
        let p = parse_invite_text(msg);
        assert_eq!(p.code.as_deref(), Some("ABCDEFGHJKMNPQRS"));
        assert_eq!(p.hosts, vec!["192.168.1.10:7443".to_string()]);

        let p = parse_invite_text("Your code is SPL-ABCD-EFGH-JKMN-PQRS. Server 10.0.0.5:7443");
        assert_eq!(p.code.as_deref(), Some("ABCDEFGHJKMNPQRS"));
        assert_eq!(p.hosts, vec!["10.0.0.5:7443".to_string()]);
    }
}
