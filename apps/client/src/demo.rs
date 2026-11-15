//! Browser preview (`index.html?demo` or `?demo=onboarding`): canned data so the UI
//! can be reviewed without the desktop shell. Never used inside the app.

use serde_json::{Value, json};
use sl_proto::client_ipc::*;
use sl_proto::*;
use uuid::Uuid;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n)
}

fn now() -> i64 {
    js_sys::Date::now() as i64
}

fn user(n: u128, name: &str, username: &str, p: Presence) -> UserLite {
    UserLite {
        id: id(n),
        display_name: name.into(),
        username: username.into(),
        presence: p,
    }
}

fn groups() -> Vec<GroupRef> {
    vec![
        GroupRef {
            id: id(900),
            name: "Operations".into(),
        },
        GroupRef {
            id: id(901),
            name: "Finance".into(),
        },
    ]
}

fn me() -> SelfUser {
    SelfUser {
        id: id(1),
        username: "alex.morgan".into(),
        display_name: "Alex Morgan".into(),
        groups: groups(),
    }
}

fn state(phase: Phase) -> AppState {
    AppState {
        phase,
        me: Some(me()),
        server: Some(PinnedServerView {
            name: "Head office".into(),
            server_id: id(500),
            fingerprint: "E0AE E89D F901 74CD 9568 C274 8447 8705".into(),
            last_address: "192.168.1.20:7443".into(),
        }),
        conn: ConnState::Connected { rtt_ms: Some(4) },
        broadcasts: vec![Broadcast {
            id: id(700),
            title: "Our usual chat is down. Use SpareLine for now.".into(),
            body: "Our cloud chat and email are unavailable this morning. Please use SpareLine for work conversations until we tell you they're back.".into(),
            severity: Severity::Warning,
            created_at: now() - 42 * 60_000,
            expires_at: None,
        }],
    }
}

fn conversations() -> Vec<ConversationView> {
    let t = now();
    let ops = GroupRef {
        id: id(900),
        name: "Operations".into(),
    };
    vec![
        ConversationView {
            id: id(100),
            kind: ConversationKind::Group,
            title: "Incident room".into(),
            scope_group: Some(ops.clone()),
            members: vec![
                user(1, "Alex Morgan", "alex.morgan", Presence::Online),
                user(2, "Priya Shah", "priya.shah", Presence::Online),
                user(3, "Sam Okafor", "sam.okafor", Presence::Away),
                user(4, "Jordan Lee", "jordan.lee", Presence::Online),
            ],
            last_message: Some(MessagePreview {
                sender_name: "Priya Shah".into(),
                text: "Vendor says ETA two hours. I'll update the status page.".into(),
                ts: t - 3 * 60_000,
            }),
            unread: 2,
            read_only: false,
            updated_at: t - 3 * 60_000,
        },
        ConversationView {
            id: id(101),
            kind: ConversationKind::Direct,
            title: "Sam Okafor".into(),
            scope_group: None,
            members: vec![
                user(1, "Alex Morgan", "alex.morgan", Presence::Online),
                user(3, "Sam Okafor", "sam.okafor", Presence::Away),
            ],
            last_message: Some(MessagePreview {
                sender_name: "You".into(),
                text: "Thanks, I'll call the bank directly.".into(),
                ts: t - 26 * 60_000,
            }),
            unread: 0,
            read_only: false,
            updated_at: t - 26 * 60_000,
        },
        ConversationView {
            id: id(102),
            kind: ConversationKind::Group,
            title: "Finance team".into(),
            scope_group: Some(GroupRef {
                id: id(901),
                name: "Finance".into(),
            }),
            members: vec![
                user(1, "Alex Morgan", "alex.morgan", Presence::Online),
                user(5, "Maria Gonzalez", "maria.g", Presence::Offline),
                user(6, "Chen Wei", "chen.wei", Presence::Online),
            ],
            last_message: Some(MessagePreview {
                sender_name: "Chen Wei".into(),
                text: "Payroll run is safe, it went out last night.".into(),
                ts: t - 3 * 3_600_000,
            }),
            unread: 0,
            read_only: false,
            updated_at: t - 3 * 3_600_000,
        },
        ConversationView {
            id: id(103),
            kind: ConversationKind::Direct,
            title: "Jordan Lee".into(),
            scope_group: None,
            members: vec![
                user(1, "Alex Morgan", "alex.morgan", Presence::Online),
                user(4, "Jordan Lee", "jordan.lee", Presence::Online),
            ],
            last_message: Some(MessagePreview {
                sender_name: "Jordan Lee".into(),
                text: "Got it 👍".into(),
                ts: t - 27 * 3_600_000,
            }),
            unread: 0,
            read_only: false,
            updated_at: t - 27 * 3_600_000,
        },
    ]
}

fn msg(
    n: u128,
    conv: Uuid,
    sender: Option<(u128, &str)>,
    body: &str,
    mins_ago: i64,
) -> MessageView {
    let mine = sender.is_some_and(|(s, _)| s == 1);
    MessageView {
        id: id(n),
        conversation: conv,
        kind: if sender.is_some() {
            MessageKind::Text
        } else {
            MessageKind::System
        },
        sender: sender.map(|(s, _)| id(s)),
        sender_name: sender.map(|(_, n)| n.to_string()).unwrap_or_default(),
        body: body.into(),
        ts: now() - mins_ago * 60_000,
        edited: false,
        deleted: false,
        mine,
        reply_to: None,
        reactions: vec![],
        status: if mine {
            SendStatus::Read
        } else {
            SendStatus::Sent
        },
    }
}

fn messages(conv: Uuid) -> Vec<MessageView> {
    if conv == id(100) {
        let me = Some((1, "Alex Morgan"));
        let priya = Some((2, "Priya Shah"));
        let sam = Some((3, "Sam Okafor"));
        let jordan = Some((4, "Jordan Lee"));
        let mut v = vec![
            msg(
                1,
                conv,
                None,
                "Jordan Lee named the chat \"Incident room\".",
                60,
            ),
            msg(
                2,
                conv,
                jordan,
                "Chat and email are both down for everyone. The provider's status page confirms a regional outage.",
                58,
            ),
            msg(
                3,
                conv,
                jordan,
                "Let's coordinate here until it's back.",
                58,
            ),
            msg(
                4,
                conv,
                me,
                "Thanks Jordan. I've sent the **announcement** so everyone sees the banner.",
                55,
            ),
            msg(
                5,
                conv,
                sam,
                "Customer calls are still coming through the phone system, so support is fine for now.",
                41,
            ),
            msg(
                6,
                conv,
                priya,
                "I've opened a ticket with the vendor: https://status.example.com/incident/4821",
                9,
            ),
            msg(
                7,
                conv,
                priya,
                "Vendor says ETA two hours. I'll update the status page.",
                3,
            ),
        ];
        v[3].reactions = vec![ReactionView {
            emoji: "👍".into(),
            count: 2,
            mine: false,
            names: vec!["Jordan Lee".into(), "Sam Okafor".into()],
        }];
        v[4].edited = true;
        v[6].reply_to = Some(ReplyPreview {
            message_id: id(5),
            sender_name: "Sam Okafor".into(),
            text: "Customer calls are still coming through…".into(),
        });
        v[5].reactions = vec![ReactionView {
            emoji: "🎉".into(),
            count: 1,
            mine: true,
            names: vec!["You".into()],
        }];
        return v;
    }
    if conv == id(101) {
        return vec![
            msg(
                20,
                conv,
                Some((3, "Sam Okafor")),
                "Do you know if the bank transfer approvals need email?",
                30,
            ),
            msg(
                21,
                conv,
                Some((1, "Alex Morgan")),
                "They don't, it's the bank's own portal. Approvals still work.",
                28,
            ),
            msg(
                22,
                conv,
                Some((3, "Sam Okafor")),
                "Great, the supplier payment can go out then.",
                27,
            ),
            msg(
                23,
                conv,
                Some((1, "Alex Morgan")),
                "Thanks, I'll call the bank directly.",
                26,
            ),
        ];
    }
    vec![msg(
        30,
        conv,
        Some((6, "Chen Wei")),
        "Payroll run is safe, it went out last night.",
        180,
    )]
}

fn directory() -> Directory {
    let u = |n: u128, name: &str, un: &str, g: Vec<u128>, p: Presence| DirectoryUser {
        id: id(n),
        username: un.into(),
        display_name: name.into(),
        groups: g.into_iter().map(id).collect(),
        presence: p,
    };
    Directory {
        groups: groups(),
        users: vec![
            u(6, "Chen Wei", "chen.wei", vec![901], Presence::Online),
            u(4, "Jordan Lee", "jordan.lee", vec![900], Presence::Online),
            u(5, "Maria Gonzalez", "maria.g", vec![901], Presence::Offline),
            u(2, "Priya Shah", "priya.shah", vec![900], Presence::Online),
            u(
                3,
                "Sam Okafor",
                "sam.okafor",
                vec![900, 901],
                Presence::Away,
            ),
        ],
    }
}

fn diagnostics() -> Diagnostics {
    Diagnostics {
        app_version: env!("CARGO_PKG_VERSION").into(),
        platform: "windows".into(),
        server: state(Phase::Ready).server,
        conn: ConnState::Connected { rtt_ms: Some(4) },
        last_attempt: vec![
            ConnStep {
                name: "Resolve address".into(),
                ok: true,
                detail: "192.168.1.20:7443".into(),
                millis: 0,
            },
            ConnStep {
                name: "Reach server (TCP)".into(),
                ok: true,
                detail: "192.168.1.20:7443".into(),
                millis: 2,
            },
            ConnStep {
                name: "Secure connection (TLS)".into(),
                ok: true,
                detail: "E0AE E89D F901 74CD 9568 C274 8447 8705".into(),
                millis: 9,
            },
            ConnStep {
                name: "Open chat channel (WebSocket)".into(),
                ok: true,
                detail: String::new(),
                millis: 3,
            },
        ],
        discovered: vec![DiscoveredServer {
            name: "Head office".into(),
            server_id: Some(id(500)),
            addresses: vec!["192.168.1.20:7443".into()],
            fingerprint_hint: Some("E0AEE89D".into()),
            source: DiscoverySource::Mdns,
        }],
        recent_failures: vec![],
        log: vec!["connected to 192.168.1.20:7443".into()],
    }
}

pub fn install(mode: &str) {
    preset_theme();
    let start = match mode {
        "onboarding" => Phase::Onboarding,
        "signin" => Phase::SignedOut,
        // "chat" = Ready, with the first conversation opened by the app.
        _ => Phase::Ready,
    };
    // Joining or signing in moves the preview into the sample workspace.
    let phase = std::rc::Rc::new(std::cell::Cell::new(start));
    sl_ui::ipc::set_mock(move |cmd, args| {
        let conv: Option<Uuid> = args
            .get("conversation")
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        let v: Value = match cmd {
            "app_state" => serde_json::to_value(state(phase.get())).unwrap_or_default(),
            "conversations" => serde_json::to_value(conversations()).unwrap_or_default(),
            "messages" => {
                let before = args.get("before").and_then(Value::as_i64);
                if before.is_some() {
                    json!([])
                } else {
                    serde_json::to_value(messages(conv.unwrap_or_default())).unwrap_or_default()
                }
            }
            "directory" => serde_json::to_value(directory()).unwrap_or_default(),
            "diagnostics" => serde_json::to_value(diagnostics()).unwrap_or_default(),
            "discover" => serde_json::to_value(diagnostics().discovered).unwrap_or_default(),
            "app_info" => {
                json!({ "version": env!("CARGO_PKG_VERSION"), "platform": "windows", "data_dir": "C:\\Users\\alex\\AppData\\Local\\SpareLine\\Client" })
            }
            "read_receipts" | "autostart_enabled" => json!(true),
            "search" => json!([]),
            "send" => {
                let input: SendInput =
                    serde_json::from_value(args.get("input").cloned().unwrap_or_default())
                        .map_err(|e| UiError {
                            code: "x".into(),
                            message: e.to_string(),
                        })?;
                let mut m = msg(
                    js_sys::Date::now() as u128,
                    input.conversation,
                    Some((1, "Alex Morgan")),
                    &input.body,
                    0,
                );
                m.status = SendStatus::Sent;
                serde_json::to_value(m).unwrap_or_default()
            }
            "join" | "login" | "redeem" => {
                phase.set(Phase::Ready);
                serde_json::to_value(state(Phase::Ready)).unwrap_or_default()
            }
            _ => Value::Null,
        };
        Ok(v)
    });
}

/// Preview only: `&theme=light|dark` presets the stored appearance before the app mounts.
fn preset_theme() {
    let Some(w) = web_sys::window() else { return };
    let search = w.location().search().unwrap_or_default();
    let theme = search
        .trim_start_matches('?')
        .split('&')
        .find_map(|p| p.strip_prefix("theme="))
        .map(str::to_string);
    if let (Some(t), Ok(Some(store))) = (theme, w.local_storage()) {
        let _ = store.set_item(
            sl_ui::prefs::STORAGE_KEY,
            &format!("{{\"theme\":\"{t}\",\"scale\":1}}"),
        );
        if let Some(el) = w.document().and_then(|d| d.document_element()) {
            let _ = el.set_attribute("data-theme", &t);
        }
    }
}
