//! Locale-aware formatting helpers (via the browser's Intl).

use wasm_bindgen::JsValue;

pub fn now() -> i64 {
    js_sys::Date::now() as i64
}

fn opts(pairs: &[(&str, &str)]) -> JsValue {
    let o = js_sys::Object::new();
    for (k, v) in pairs {
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str(k), &JsValue::from_str(v));
    }
    o.into()
}

fn date(ts: i64) -> js_sys::Date {
    js_sys::Date::new(&JsValue::from_f64(ts as f64))
}

/// "14:05" or "2:05 PM", following the system locale.
pub fn time(ts: i64) -> String {
    date(ts)
        .to_locale_time_string_with_options(
            "default",
            &opts(&[("hour", "numeric"), ("minute", "2-digit")]),
        )
        .into()
}

fn day_start(ts: i64) -> i64 {
    let d = date(ts);
    d.set_hours(0);
    d.set_minutes(0);
    d.set_seconds(0);
    d.set_milliseconds(0);
    d.get_time() as i64
}

/// "Today", "Yesterday", "Monday", or "12 March 2026".
pub fn day_label(ts: i64) -> String {
    let today = day_start(now());
    let day = day_start(ts);
    let diff = (today - day) / 86_400_000;
    match diff {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => date(ts)
            .to_locale_date_string("default", &opts(&[("weekday", "long")]))
            .into(),
        _ => date(ts)
            .to_locale_date_string(
                "default",
                &opts(&[("day", "numeric"), ("month", "long"), ("year", "numeric")]),
            )
            .into(),
    }
}

pub fn same_day(a: i64, b: i64) -> bool {
    day_start(a) == day_start(b)
}

/// Compact timestamp for lists: time today, "Yesterday", weekday, or short date.
pub fn short_when(ts: i64) -> String {
    let today = day_start(now());
    let diff = (today - day_start(ts)) / 86_400_000;
    match diff {
        0 => time(ts),
        1 => "Yesterday".into(),
        2..=6 => date(ts)
            .to_locale_date_string("default", &opts(&[("weekday", "short")]))
            .into(),
        _ => date(ts)
            .to_locale_date_string("default", &opts(&[("day", "numeric"), ("month", "short")]))
            .into(),
    }
}

/// Full date and time, for tooltips and tables.
pub fn full(ts: i64) -> String {
    let d = date(ts);
    let day: String = d
        .to_locale_date_string(
            "default",
            &opts(&[("day", "numeric"), ("month", "short"), ("year", "numeric")]),
        )
        .into();
    format!("{day}, {}", time(ts))
}

/// "just now", "4 min ago", "3 h ago", "2 days ago".
pub fn ago(ts: i64) -> String {
    let s = (now() - ts) / 1000;
    if s < 45 {
        "just now".into()
    } else if s < 3600 {
        format!("{} min ago", (s / 60).max(1))
    } else if s < 86_400 {
        format!("{} h ago", s / 3600)
    } else {
        let d = s / 86_400;
        if d == 1 {
            "1 day ago".into()
        } else {
            format!("{d} days ago")
        }
    }
}

/// "in 6 days", "in 3 h", "expired".
pub fn until(ts: i64) -> String {
    let s = (ts - now()) / 1000;
    if s <= 0 {
        "expired".into()
    } else if s < 3600 {
        format!("in {} min", (s / 60).max(1))
    } else if s < 172_800 {
        format!("in {} h", s / 3600)
    } else {
        format!("in {} days", s / 86_400)
    }
}

pub fn duration(ms: i64) -> String {
    let s = ms / 1000;
    let (d, h, m) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{} min", m.max(1))
    }
}

pub fn bytes(b: u64) -> String {
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

/// "Alex Smith" → "AS", "alex" → "AL".
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let s: String = match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect(),
        [first, .., last] => first.chars().take(1).chain(last.chars().take(1)).collect(),
    };
    s.to_uppercase()
}

/// Stable 0–7 hue bucket for avatar colours.
pub fn hue(seed: &str) -> u8 {
    let mut h: u32 = 2166136261;
    for b in seed.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(16777619);
    }
    (h % 8) as u8
}

pub fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}
