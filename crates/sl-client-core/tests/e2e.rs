//! End-to-end: an in-process server and several headless clients.

use std::sync::Arc;
use std::time::Duration;

use sl_client_core::{Client, ClientOptions, MemoryStore};
use sl_proto::client_ipc::*;
use sl_proto::panel::CreateInvitesInput;
use sl_proto::{ConversationKind, ErrorCode, UserId};
use sl_server::{Engine, EngineOptions, Telemetry};
use tokio::sync::broadcast;

const T: Duration = Duration::from_secs(15);

fn client(dir: &std::path::Path, name: &str) -> Client {
    Client::start(ClientOptions {
        dir: dir.join(name),
        secrets: Arc::new(MemoryStore::default()),
        device_name: format!("{name}-laptop"),
        platform: "test".into(),
        client_version: "0.1.0-test".into(),
        discovery: false,
    })
    .expect("client starts")
}

async fn wait_for<F: FnMut(&ClientEvent) -> bool>(
    rx: &mut broadcast::Receiver<ClientEvent>,
    what: &str,
    mut f: F,
) -> ClientEvent {
    let deadline = tokio::time::Instant::now() + T;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(e)) if f(&e) => return e,
            Ok(Ok(_)) => {}
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(e)) => panic!("event stream closed while waiting for {what}: {e}"),
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

async fn wait_message(rx: &mut broadcast::Receiver<ClientEvent>, body: &str) -> MessageView {
    match wait_for(
        rx,
        body,
        |e| matches!(e, ClientEvent::Message(m) if m.body == body),
    )
    .await
    {
        ClientEvent::Message(m) => m,
        _ => unreachable!(),
    }
}

async fn invite(engine: &Engine, groups: Vec<uuid::Uuid>) -> String {
    let created = engine
        .admin()
        .create_invites(CreateInvitesInput {
            count: 1,
            group_ids: groups,
            expires_in_secs: 3600,
            note: String::new(),
        })
        .await
        .expect("invite");
    created[0].message.clone()
}

async fn join(c: &Client, invite_text: String, addr: &str, username: &str) -> AppState {
    c.join(JoinInput {
        invite_text,
        address: addr.to_string(),
        username: username.into(),
        display_name: format!("{}{}", username[..1].to_uppercase(), &username[1..]),
        password: "correct horse battery".into(),
    })
    .await
    .unwrap_or_else(|e| panic!("{username} join: {e}"))
}

fn user_id(state: &AppState) -> UserId {
    state.me.as_ref().expect("signed in").id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_flow() {
    let tmp = tempfile::tempdir().expect("tmp");
    let mut opts = EngineOptions::new(tmp.path().join("server"), Telemetry::detached());
    opts.bind_override = Some("127.0.0.1:0".parse().expect("addr"));
    opts.mdns = false;
    let engine = Engine::open(opts).expect("engine");
    assert!(engine.needs_setup());
    let recovery = engine
        .setup("Test Office", "admin password 1")
        .await
        .expect("setup");
    assert!(recovery.starts_with("RK-"));
    let local = engine.start().await.expect("start");
    let addr = format!("127.0.0.1:{}", local.port());

    let admin = engine.admin();
    let ops = admin
        .create_group("Ops".into(), String::new())
        .await
        .expect("group")
        .id;
    let fin = admin
        .create_group("Finance".into(), String::new())
        .await
        .expect("group")
        .id;

    // --- onboarding -------------------------------------------------------------------
    let alice = client(tmp.path(), "alice");
    let bob = client(tmp.path(), "bob");
    let carol = client(tmp.path(), "carol");
    assert_eq!(alice.state().phase, Phase::Onboarding);

    // A wrong code is rejected before anything is pinned.
    let bad = alice
        .join(JoinInput {
            invite_text: "SPL-0000-0000-0000-0000".into(),
            address: addr.clone(),
            username: "alice".into(),
            display_name: "Alice".into(),
            password: "correct horse battery".into(),
        })
        .await;
    assert!(matches!(bad, Err(e) if e.api_code() == Some(ErrorCode::InvalidInvite)));

    let a_state = join(&alice, invite(&engine, vec![ops]).await, &addr, "alice").await;
    let b_state = join(&bob, invite(&engine, vec![ops, fin]).await, &addr, "bob").await;
    let c_state = join(&carol, invite(&engine, vec![fin]).await, &addr, "carol").await;
    assert_eq!(a_state.phase, Phase::Ready);
    let (a_id, b_id, c_id) = (user_id(&a_state), user_id(&b_state), user_id(&c_state));

    // An invite works only once.
    let used = invite(&engine, vec![ops]).await;
    let dave = client(tmp.path(), "dave");
    join(&dave, used.clone(), &addr, "dave").await;
    let eve = client(tmp.path(), "eve");
    let again = eve
        .join(JoinInput {
            invite_text: used,
            address: addr.clone(),
            username: "eve".into(),
            display_name: "Eve".into(),
            password: "correct horse battery".into(),
        })
        .await;
    assert!(matches!(again, Err(e) if e.api_code() == Some(ErrorCode::InviteUsed)));

    // --- group isolation --------------------------------------------------------------
    let dir_a = alice.directory().await.expect("dir");
    let seen: Vec<UserId> = dir_a.users.iter().map(|u| u.id).collect();
    assert!(seen.contains(&b_id), "alice sees bob (both in Ops)");
    assert!(
        !seen.contains(&c_id),
        "alice must not see carol (no shared group)"
    );
    let dir_c = carol.directory().await.expect("dir");
    assert!(dir_c.users.iter().any(|u| u.id == b_id));
    assert!(!dir_c.users.iter().any(|u| u.id == a_id));

    let forbidden = alice
        .start_chat(StartChatInput {
            kind: ConversationKind::Direct,
            members: vec![c_id],
            scope_group: None,
            title: String::new(),
        })
        .await;
    assert!(forbidden.is_err(), "alice cannot start a chat with carol");

    // --- direct chat, end-to-end encrypted ------------------------------------------
    let mut bob_rx = bob.subscribe();
    let mut alice_rx = alice.subscribe();
    let dm = alice
        .start_chat(StartChatInput {
            kind: ConversationKind::Direct,
            members: vec![b_id],
            scope_group: None,
            title: String::new(),
        })
        .await
        .expect("dm");
    assert_eq!(dm.title, "Bob");
    let sent = alice
        .send(SendInput {
            conversation: dm.id,
            body: "hello bob, quarterly numbers attached".into(),
            reply_to: None,
        })
        .await
        .expect("send");
    assert_eq!(sent.status, SendStatus::Pending);
    let got = wait_message(&mut bob_rx, "hello bob, quarterly numbers attached").await;
    assert_eq!(got.sender, Some(a_id));
    assert!(!got.mine);

    // Reply, reaction, edit.
    bob.send(SendInput {
        conversation: dm.id,
        body: "thanks alice".into(),
        reply_to: Some(got.id),
    })
    .await
    .expect("reply");
    let reply = wait_message(&mut alice_rx, "thanks alice").await;
    assert_eq!(reply.reply_to.as_ref().map(|r| r.message_id), Some(sent.id));
    bob.react(dm.id, got.id, "👍".into()).await.expect("react");
    wait_for(&mut alice_rx, "reaction", |e| matches!(e, ClientEvent::Message(m) if m.id == sent.id && m.reactions.iter().any(|r| r.emoji == "👍"))).await;
    alice
        .edit(dm.id, sent.id, "hello bob (edited)".into())
        .await
        .expect("edit");
    let edited = wait_message(&mut bob_rx, "hello bob (edited)").await;
    assert!(edited.edited);

    // Bob can't edit Alice's message.
    assert!(bob.edit(dm.id, sent.id, "hacked".into()).await.is_err());

    // Read receipt: Bob reads, Alice's message shows as read.
    bob.mark_read(dm.id).await.expect("read");
    wait_for(
        &mut alice_rx,
        "read receipt",
        |e| matches!(e, ClientEvent::Message(m) if m.id == sent.id && m.status == SendStatus::Read),
    )
    .await;

    // Starting the same direct chat again returns the existing one.
    let again = bob
        .start_chat(StartChatInput {
            kind: ConversationKind::Direct,
            members: vec![a_id],
            scope_group: None,
            title: String::new(),
        })
        .await
        .expect("existing dm");
    assert_eq!(again.id, dm.id);

    // --- group chat scoped to Finance --------------------------------------------------
    let mut carol_rx = carol.subscribe();
    let grp = bob
        .start_chat(StartChatInput {
            kind: ConversationKind::Group,
            members: vec![c_id],
            scope_group: Some(fin),
            title: "Finance team".into(),
        })
        .await
        .expect("group");
    assert_eq!(grp.title, "Finance team");
    // Alice is not in Finance, so she can't be added.
    assert!(bob.add_people(grp.id, vec![a_id]).await.is_err());
    bob.send(SendInput {
        conversation: grp.id,
        body: "budget review at 3".into(),
        reply_to: None,
    })
    .await
    .expect("send");
    wait_message(&mut carol_rx, "budget review at 3").await;
    wait_for(&mut carol_rx, "group title", |e| matches!(e, ClientEvent::Conversation(c) if c.id == grp.id && c.title == "Finance team")).await;

    // --- admin removes Bob from Finance: he loses the chat ------------------------------
    admin
        .set_user_groups(b_id, vec![ops])
        .await
        .expect("regroup");
    wait_for(
        &mut bob_rx,
        "removed from group chat",
        |e| matches!(e, ClientEvent::ConversationRemoved { id } if *id == grp.id),
    )
    .await;
    // Carol's client commits the MLS removal; then her message must not reach Bob.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    carol
        .send(SendInput {
            conversation: grp.id,
            body: "bob should not see this".into(),
            reply_to: None,
        })
        .await
        .expect("send");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let bob_msgs = bob.messages(grp.id, None, 100).await.expect("msgs");
    assert!(!bob_msgs.iter().any(|m| m.body.contains("should not see")));
    let carol_msgs = carol.messages(grp.id, None, 100).await.expect("msgs");
    let mine = carol_msgs
        .iter()
        .find(|m| m.body == "bob should not see this")
        .expect("carol has her message");
    assert_eq!(
        mine.status,
        SendStatus::Sent,
        "carol's message was accepted after the removal"
    );

    // --- new device via a setup code ---------------------------------------------------
    let setup = admin
        .create_setup_code(a_id, 3600)
        .await
        .expect("setup code");
    let alice2 = client(tmp.path(), "alice2");
    let st = alice2
        .redeem(RedeemInput {
            invite_text: setup.code.clone(),
            address: addr.clone(),
            new_password: "a brand new password".into(),
        })
        .await
        .expect("redeem");
    assert_eq!(user_id(&st), a_id);
    let mut alice2_rx = alice2.subscribe();
    // Give the new device a moment to join the DM by external commit.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    bob.send(SendInput {
        conversation: dm.id,
        body: "welcome back alice".into(),
        reply_to: None,
    })
    .await
    .expect("send");
    wait_message(&mut alice2_rx, "welcome back alice").await;

    // --- privacy: no plaintext on the server ------------------------------------------
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut raw = Vec::new();
    for f in ["spareline.db", "spareline.db-wal"] {
        raw.extend(std::fs::read(tmp.path().join("server").join(f)).unwrap_or_default());
    }
    assert!(raw.len() > 10_000, "server database should have content");
    for secret in [
        "quarterly",
        "thanks alice",
        "budget review",
        "Finance team",
        "welcome back",
        "should not see",
    ] {
        assert!(
            !raw.windows(secret.len()).any(|w| w == secret.as_bytes()),
            "server DB must not contain {secret:?}"
        );
    }

    engine.stop().await;
}
