//! A small, consistent icon set (24×24, 1.8 stroke, round joins). Decorative by
//! default: pair with visible text or an `aria-label` on the control.

use leptos::prelude::*;

fn paths(name: &str) -> &'static str {
    match name {
        "chat" => r#"<path d="M4 5.5h16v10.5H9.5L4 20z"/>"#,
        "people" => {
            r#"<circle cx="9" cy="8.5" r="3.2"/><path d="M3.5 19.5c.6-3.3 2.9-5.2 5.5-5.2s4.9 1.9 5.5 5.2"/><circle cx="17" cy="9.5" r="2.4"/><path d="M15.6 14.4c2.4-.2 4.3 1.3 4.9 4.3"/>"#
        }
        "settings" => {
            r#"<path d="M4 7h9M18 7h2M4 17h3M12 17h8"/><circle cx="15.5" cy="7" r="2.3"/><circle cx="9.5" cy="17" r="2.3"/>"#
        }
        "plus" => r#"<path d="M12 5v14M5 12h14"/>"#,
        "search" => r#"<circle cx="11" cy="11" r="6"/><path d="m20 20-4.3-4.3"/>"#,
        "send" => r#"<path d="M4.5 11.5 20 4.5l-6.8 15.5-2.4-6.3z"/><path d="M10.8 13.7 20 4.5"/>"#,
        "smile" => {
            r#"<circle cx="12" cy="12" r="8.2"/><path d="M8.6 14.2c.9 1.3 2.1 2 3.4 2s2.5-.7 3.4-2"/><path d="M9.2 10h.01M14.8 10h.01"/>"#
        }
        "reply" => r#"<path d="M10 7 5 12l5 5"/><path d="M5 12h9a5 5 0 0 1 5 5v1"/>"#,
        "edit" => r#"<path d="M5 19h4.2L19.5 8.7l-4.2-4.2L5 14.8z"/><path d="m13.2 6.6 4.2 4.2"/>"#,
        "trash" => r#"<path d="M5 7h14M10 7V4.8h4V7M7 7l1 12.2h8L17 7"/>"#,
        "more" => r#"<path d="M6 12h.01M12 12h.01M18 12h.01" stroke-width="3"/>"#,
        "close" => r#"<path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/>"#,
        "check" => r#"<path d="m5 12.5 4.5 4.5L19 7.5"/>"#,
        "check2" => r#"<path d="m2.8 12.8 4.4 4.4L16 8.4"/><path d="m11.6 16.2 1 1L21.4 8.4"/>"#,
        "clock" => r#"<circle cx="12" cy="12" r="8.2"/><path d="M12 7.8v4.6l3 1.6"/>"#,
        "alert" => r#"<path d="M12 4 2.8 19.5h18.4z"/><path d="M12 10v4.4M12 17h.01"/>"#,
        "info" => r#"<circle cx="12" cy="12" r="8.5"/><path d="M12 11v5.5M12 7.6h.01"/>"#,
        "megaphone" => {
            r#"<path d="M4 10v4h3l7.5 4.5v-13L7 10z"/><path d="M17.8 9.4a3.6 3.6 0 0 1 0 5.2"/>"#
        }
        "server" => {
            r#"<rect x="4" y="4" width="16" height="7" rx="1.6"/><rect x="4" y="13" width="16" height="7" rx="1.6"/><path d="M8 7.5h.01M8 16.5h.01"/>"#
        }
        "shield" => {
            r#"<path d="M12 3.5 5 6.2v5.4c0 4.3 2.9 7.6 7 8.9 4.1-1.3 7-4.6 7-8.9V6.2z"/><path d="m9 12 2 2 4-4"/>"#
        }
        "key" => r#"<circle cx="8" cy="15" r="3.6"/><path d="m10.6 12.4 8-8M16 7l2.2 2.2"/>"#,
        "pulse" => r#"<path d="M3 12h4l2.5-6.5 5 13L17 12h4"/>"#,
        "list" => r#"<path d="M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01"/>"#,
        "tool" => {
            r#"<path d="M14.7 6.3a4 4 0 0 0 5 5L12 19a2.1 2.1 0 1 1-3-3l7.6-7.6"/><path d="M14.7 6.3 17 4"/>"#
        }
        "invite" => {
            r#"<circle cx="10" cy="8.5" r="3.2"/><path d="M4 19.5c.6-3.3 3-5.2 6-5.2 1.4 0 2.6.4 3.6 1"/><path d="M18 13v6.5M14.8 16.2h6.4"/>"#
        }
        "groups" => {
            r#"<rect x="4" y="4" width="7" height="7" rx="1.6"/><rect x="13" y="4" width="7" height="7" rx="1.6"/><rect x="4" y="13" width="7" height="7" rx="1.6"/><rect x="13" y="13" width="7" height="7" rx="1.6"/>"#
        }
        "home" => r#"<path d="M4 11 12 4.2l8 6.8v9h-5.2v-5.5H9.2V20H4z"/>"#,
        "copy" => {
            r#"<rect x="8.5" y="8.5" width="11.5" height="11.5" rx="2"/><path d="M15.5 8.5V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v9.5a1 1 0 0 0 1 1h3.5"/>"#
        }
        "refresh" => {
            r#"<path d="M19.5 10.5A7.8 7.8 0 0 0 5.7 7L4 8.6M4 4.5v4.1h4.1"/><path d="M4.5 13.5A7.8 7.8 0 0 0 18.3 17l1.7-1.6M20 19.5v-4.1h-4.1"/>"#
        }
        "logout" => r#"<path d="M10 4.5H5v15h5M15 8l4 4-4 4M19 12H9.5"/>"#,
        "lock" => {
            r#"<rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>"#
        }
        "download" => r#"<path d="M12 4v11M7 10.5l5 4.5 5-4.5M5 20h14"/>"#,
        "printer" => r#"<path d="M7 9V4h10v5M7 17H5v-7.5h14V17h-2M7 14h10v6H7z"/>"#,
        "chevron-down" => r#"<path d="m6 9.5 6 6 6-6"/>"#,
        "chevron-left" => r#"<path d="m15 6-6 6 6 6"/>"#,
        "chevron-right" => r#"<path d="m9 6 6 6-6 6"/>"#,
        "bug" => {
            r#"<rect x="8" y="7.5" width="8" height="12" rx="4"/><path d="M12 7.5V5.5M8 11.5H4.5M19.5 11.5H16M8 16H5M19 16h-3M9.6 5.4 8.2 4M14.4 5.4 15.8 4"/>"#
        }
        "offline" => {
            r#"<path d="M3.5 3.5l17 17M8.6 16.4a5 5 0 0 1 6.2-.6M5.2 13a9.8 9.8 0 0 1 4.2-2.4M14.5 10.6A9.8 9.8 0 0 1 18.8 13M2 9.6A14.7 14.7 0 0 1 6.4 6.8M10.4 5.6A14.7 14.7 0 0 1 22 9.6"/><path d="M12 20h.01"/>"#
        }
        "link" => {
            r#"<path d="M10 14a4.5 4.5 0 0 0 6.4 0l3-3a4.5 4.5 0 0 0-6.4-6.4l-1.2 1.2"/><path d="M14 10a4.5 4.5 0 0 0-6.4 0l-3 3a4.5 4.5 0 0 0 6.4 6.4l1.2-1.2"/>"#
        }
        "power" => r#"<path d="M12 3.5v8"/><path d="M7.2 6.5a7.5 7.5 0 1 0 9.6 0"/>"#,
        "eye" => {
            r#"<path d="M2.8 12S6 5.8 12 5.8 21.2 12 21.2 12 18 18.2 12 18.2 2.8 12 2.8 12z"/><circle cx="12" cy="12" r="2.8"/>"#
        }
        _ => r#"<circle cx="12" cy="12" r="2"/>"#,
    }
}

/// `<Icon name="chat"/>`. Always `aria-hidden`; label the control instead.
#[component]
pub fn Icon(name: &'static str, #[prop(optional)] class: &'static str) -> impl IntoView {
    let class = if class.is_empty() {
        "icon".to_string()
    } else {
        format!("icon {class}")
    };
    view! {
        <svg
            class=class
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
            focusable="false"
            inner_html=paths(name)
        ></svg>
    }
}
