//! One version for the whole project.
//!
//! The version lives in one place: `version` under `[workspace.package]` in the root
//! `Cargo.toml`. Every crate inherits it, both Tauri apps read it at build time (their
//! `tauri.conf.json` files deliberately have no `version`), and the Rust code reports
//! it through `env!("CARGO_PKG_VERSION")`.
//!
//! A few text files repeat it for readers: `CITATION.cff`, the README footer (between
//! `<!-- version -->` markers) and a heading in `CHANGELOG.md`. `cargo xtask version
//! set` and `bump` update all of them together, and `cargo xtask ci` fails if any of
//! them disagree.
//!
//! ```text
//! cargo xtask version                        show the version and check every file agrees
//! cargo xtask version bump <patch|minor|major>
//! cargo xtask version set <X.Y.Z>
//! ```

use std::fmt;
use std::fs;
use std::ops::Range;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const CARGO_TOML: &str = "Cargo.toml";
const CITATION: &str = "CITATION.cff";
const README: &str = "README.md";
const CHANGELOG: &str = "CHANGELOG.md";
const TAURI_CONFIGS: [&str; 2] = [
    "apps/server/src-tauri/tauri.conf.json",
    "apps/client/src-tauri/tauri.conf.json",
];
const MARK_OPEN: &str = "<!-- version -->";
const MARK_CLOSE: &str = "<!-- /version -->";
const UNRELEASED: &str = "## Unreleased";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Version {
    /// `X.Y.Z` with no leading zeros, as semver requires.
    pub fn parse(s: &str) -> Option<Self> {
        let num = |p: &str| -> Option<u32> {
            if p.is_empty() || (p.len() > 1 && p.starts_with('0')) {
                return None;
            }
            if !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            p.parse().ok()
        };
        let mut parts = s.trim().split('.');
        let v = Self {
            major: num(parts.next()?)?,
            minor: num(parts.next()?)?,
            patch: num(parts.next()?)?,
        };
        parts.next().is_none().then_some(v)
    }

    pub fn bump(self, part: &str) -> Option<Self> {
        Some(match part {
            "patch" => Self {
                patch: self.patch + 1,
                ..self
            },
            "minor" => Self {
                minor: self.minor + 1,
                patch: 0,
                ..self
            },
            "major" => Self {
                major: self.major + 1,
                minor: 0,
                patch: 0,
            },
            _ => return None,
        })
    }

    /// Windows installers (MSI) only accept major and minor up to 255 and patch up to
    /// 65535, so a version outside that can't be bundled.
    fn installable(self) -> bool {
        self.major <= 255 && self.minor <= 255 && self.patch <= 65_535
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

// ---------------------------------------------------------------------------------------
// Finding the version in each file
// ---------------------------------------------------------------------------------------

/// Where the quoted value of `version` under `[workspace.package]` sits.
fn cargo_version_span(text: &str) -> Option<Range<usize>> {
    let mut in_section = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t == "[workspace.package]";
        } else if in_section
            && t.strip_prefix("version")
                .is_some_and(|r| r.trim_start().starts_with('='))
        {
            let open = line.find('"')? + 1;
            let close = open + line[open..].find('"')?;
            return Some(offset + open..offset + close);
        }
        offset += line.len();
    }
    None
}

/// The text after `key` on the first line that starts with it.
fn line_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|l| l.strip_prefix(key))
        .map(|v| v.trim().trim_matches('"'))
}

/// Replace the whole line that starts with `key`.
fn replace_line(text: &str, key: &str, new_line: &str) -> Option<String> {
    let mut found = false;
    let out: Vec<String> = text
        .split_inclusive('\n')
        .map(|l| {
            if !found && l.starts_with(key) {
                found = true;
                let end = if l.ends_with('\n') { "\n" } else { "" };
                format!("{new_line}{end}")
            } else {
                l.to_string()
            }
        })
        .collect();
    found.then(|| out.concat())
}

fn between_markers(text: &str) -> Option<Range<usize>> {
    let start = text.find(MARK_OPEN)? + MARK_OPEN.len();
    let end = start + text[start..].find(MARK_CLOSE)?;
    Some(start..end)
}

fn changelog_heading(v: Version) -> String {
    format!("## {v} (")
}

fn read(path: &str) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("can't read {path}: {e}"))
}

fn write(path: &str, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|e| format!("can't write {path}: {e}"))
}

pub fn current() -> Result<Version, String> {
    let text = read(CARGO_TOML)?;
    let span =
        cargo_version_span(&text).ok_or("no version under [workspace.package] in Cargo.toml")?;
    Version::parse(&text[span.clone()])
        .ok_or_else(|| format!("Cargo.toml has an invalid version: {}", &text[span]))
}

// ---------------------------------------------------------------------------------------
// Check
// ---------------------------------------------------------------------------------------

/// Every place that repeats the version, compared with Cargo.toml.
pub fn problems() -> Vec<String> {
    let v = match current() {
        Ok(v) => v,
        Err(e) => return vec![e],
    };
    let mut out = Vec::new();
    let want = v.to_string();
    match read(CITATION) {
        Ok(t) => match line_value(&t, "version:") {
            Some(got) if got == want => {}
            got => out.push(format!(
                "{CITATION} says version {}, Cargo.toml says {want}",
                got.unwrap_or("nothing")
            )),
        },
        Err(e) => out.push(e),
    }
    match read(README) {
        Ok(t) => match between_markers(&t).map(|r| t[r].trim().to_string()) {
            Some(got) if got == want => {}
            Some(got) => out.push(format!(
                "{README} says version {got}, Cargo.toml says {want}"
            )),
            None => out.push(format!("{README} has no {MARK_OPEN}…{MARK_CLOSE} markers")),
        },
        Err(e) => out.push(e),
    }
    match read(CHANGELOG) {
        Ok(t) if t.contains(&changelog_heading(v)) => {}
        Ok(_) => out.push(format!("{CHANGELOG} has no \"## {want} (date)\" heading")),
        Err(e) => out.push(e),
    }
    for path in TAURI_CONFIGS {
        match read(path) {
            Ok(t) if t.contains("\"version\":") => out.push(format!(
                "{path} sets its own version; remove it so the app uses the one in Cargo.toml"
            )),
            Ok(_) => {}
            Err(e) => out.push(e),
        }
    }
    if !v.installable() {
        out.push(format!(
            "{want} can't be used for Windows installers (major and minor must be 255 or less, patch 65535 or less)"
        ));
    }
    out
}

/// For `cargo xtask ci`.
pub fn check() -> bool {
    let p = problems();
    if p.is_empty() {
        if let Ok(v) = current() {
            println!("version {v}: every file agrees");
        }
        return true;
    }
    eprintln!("Version check failed:");
    for e in &p {
        eprintln!("  - {e}");
    }
    eprintln!("Run `cargo xtask version set <X.Y.Z>` to bring them back in line.");
    false
}

// ---------------------------------------------------------------------------------------
// Set
// ---------------------------------------------------------------------------------------

/// Today's date in UTC, as YYYY-MM-DD.
fn today() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn set(new: Version) -> Result<(), String> {
    if !new.installable() {
        return Err(format!(
            "{new} can't be used for Windows installers (major and minor must be 255 or less, patch 65535 or less)"
        ));
    }
    let old = current()?;
    let date = today();
    let v = new.to_string();

    let mut cargo = read(CARGO_TOML)?;
    let span = cargo_version_span(&cargo).ok_or("no workspace version in Cargo.toml")?;
    cargo.replace_range(span, &v);

    let citation = read(CITATION)?;
    let citation = replace_line(&citation, "version:", &format!("version: \"{v}\""))
        .ok_or(format!("no `version:` line in {CITATION}"))?;
    let citation = replace_line(
        &citation,
        "date-released:",
        &format!("date-released: {date}"),
    )
    .ok_or(format!("no `date-released:` line in {CITATION}"))?;

    let mut readme = read(README)?;
    let r = between_markers(&readme)
        .ok_or(format!("{README} has no {MARK_OPEN}…{MARK_CLOSE} markers"))?;
    readme.replace_range(r, &v);

    let mut changelog = read(CHANGELOG)?;
    let mut empty_notes = false;
    if !changelog.contains(&changelog_heading(new)) {
        let at = changelog
            .find(UNRELEASED)
            .ok_or(format!("no \"{UNRELEASED}\" heading in {CHANGELOG}"))?;
        let line_end = at + changelog[at..].find('\n').unwrap_or(changelog.len() - at);
        let after = &changelog[line_end..];
        empty_notes = after.trim_start().starts_with("## ") || after.trim().is_empty();
        changelog.insert_str(line_end, &format!("\n\n## {v} ({date})"));
    }

    for path in TAURI_CONFIGS {
        let t = read(path)?;
        if t.contains("\"version\":") {
            return Err(format!(
                "{path} sets its own version; remove that line so the app uses Cargo.toml"
            ));
        }
    }

    write(CARGO_TOML, &cargo)?;
    write(CITATION, &citation)?;
    write(README, &readme)?;
    write(CHANGELOG, &changelog)?;

    // Refresh Cargo.lock so it records the new version for every workspace crate.
    let lock_ok = Command::new("cargo")
        .args(["metadata", "--offline", "--format-version", "1"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());

    println!("Version {old} → {v}");
    println!("  updated Cargo.toml, {CITATION}, {README} and {CHANGELOG}");
    if !lock_ok {
        println!("  couldn't refresh Cargo.lock; the next cargo build will do it");
    }
    if empty_notes {
        println!("  {CHANGELOG} had nothing under \"{UNRELEASED}\": add notes under \"## {v}\"");
    }
    println!();
    println!("To release it:");
    println!("  git commit -am \"Release {v}\"");
    println!("  git tag -a v{v} -m \"SpareLine {v}\"");
    println!("  git push --follow-tags");
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------

const USAGE: &str = "\
usage:
  cargo xtask version                        show the version and check every file agrees
  cargo xtask version bump <patch|minor|major>
  cargo xtask version set <X.Y.Z>";

pub fn run(args: &[String]) -> bool {
    let result = match args.first().map(String::as_str) {
        None | Some("show") => {
            return match current() {
                Ok(v) => {
                    println!("{v}");
                    check()
                }
                Err(e) => {
                    eprintln!("{e}");
                    false
                }
            };
        }
        Some("bump") => match args.get(1) {
            Some(part) => current().and_then(|v| {
                v.bump(part)
                    .ok_or(format!("bump takes patch, minor or major, not \"{part}\""))
                    .and_then(set)
            }),
            None => Err("bump takes patch, minor or major".into()),
        },
        Some("set") => match args.get(1).map(|s| (s, Version::parse(s))) {
            Some((_, Some(v))) => set(v),
            Some((s, None)) => Err(format!("\"{s}\" isn't a version like 0.1.22")),
            None => Err("set takes a version like 0.1.22".into()),
        },
        Some(other) => Err(format!("unknown version command \"{other}\"")),
    };
    match result {
        Ok(()) => true,
        Err(e) => {
            eprintln!("{e}");
            eprintln!("{USAGE}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_bumps() {
        let v = Version::parse("0.1.22").expect("valid");
        assert_eq!(v.to_string(), "0.1.22");
        assert_eq!(
            v.bump("patch").map(|v| v.to_string()).as_deref(),
            Some("0.1.23")
        );
        assert_eq!(
            v.bump("minor").map(|v| v.to_string()).as_deref(),
            Some("0.2.0")
        );
        assert_eq!(
            v.bump("major").map(|v| v.to_string()).as_deref(),
            Some("1.0.0")
        );
        assert!(v.bump("build").is_none());
        for bad in ["0.1", "0.1.2.3", "01.1.2", "0.1.x", "", "v0.1.2", "0.1.-2"] {
            assert!(Version::parse(bad).is_none(), "{bad} should be rejected");
        }
        assert!(!Version::parse("0.256.0").expect("valid").installable());
    }

    #[test]
    fn finds_the_workspace_version() {
        let toml = "[package]\nversion = \"9.9.9\"\n\n[workspace.package]\nedition = \"2024\"\nversion = \"0.1.22\"\n\n[dependencies]\nversion = \"1\"\n";
        let span = cargo_version_span(toml).expect("found");
        assert_eq!(&toml[span], "0.1.22");
    }

    #[test]
    fn replaces_lines_and_markers() {
        let cff = "cff-version: 1.2.0\nversion: \"0.1.0\"\ndate-released: 2023-02-21\n";
        let out = replace_line(cff, "version:", "version: \"0.1.22\"").expect("found");
        assert_eq!(line_value(&out, "version:"), Some("0.1.22"));
        assert_eq!(line_value(&out, "cff-version:"), Some("1.2.0"));
        let md = "early development, version <!-- version -->0.1.0<!-- /version -->.";
        let r = between_markers(md).expect("markers");
        assert_eq!(&md[r], "0.1.0");
    }

    #[test]
    fn today_is_a_date() {
        let d = today();
        assert_eq!(d.len(), 10);
        assert!(d.starts_with("20"));
    }
}
