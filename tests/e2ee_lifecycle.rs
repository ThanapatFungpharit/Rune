//! Letter Sealing failure handling, restart and key changes, against a mock
//! gateway with *real* keychain keys from a session file (made-up mids are
//! rejected by `ltsm.wasm`, so our own mid plays the peer). No traffic
//! reaches LINE.
//!
//! ```sh
//! RUNE_SESSION=session.json cargo test --test e2ee_lifecycle -- --ignored
//! ```

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use rune::ltsm::{KeyHandle, Ltsm};
use rune::models::Operation;
use rune::{Client, Config, E2eeErrorKind, Error, Message, OutgoingMessage, Session, Tokens};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn ok(data: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"message": "OK", "data": data}))
}

fn talk_error(code: i64) -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(
        json!({"code": 10051, "data": {"name": "TalkException", "code": code, "reason": "mock"}}),
    )
}

/// Our three real keys: `[oldest, middle, latest]` as (id, public key, handle in `ltsm`).
struct Keys {
    me: String,
    session: Session,
    ltsm: Ltsm,
    k: Vec<(i64, Vec<u8>, KeyHandle)>,
}

fn keys() -> Keys {
    let s = Session::load(std::env::var("RUNE_SESSION").expect("set RUNE_SESSION")).unwrap();
    let me = s.tokens.mid.clone().unwrap();
    let mut blobs = s.e2ee_keys.clone();
    blobs.sort_by_key(|k| k.key_id);
    assert!(blobs.len() >= 3, "need three keys in the session");
    let mut ltsm = Ltsm::new().unwrap();
    let k = blobs
        .iter()
        .map(|b| {
            let h = ltsm.load_e2ee_key(&B64.decode(&b.exported).unwrap()).unwrap();
            (b.key_id as i64, ltsm.public_key(h).unwrap(), h)
        })
        .collect();
    let session = Session {
        tokens: Tokens {
            access_token: Some("A".into()),
            mid: Some(me.clone()),
            ..Default::default()
        },
        e2ee_keys: s.e2ee_keys,
    };
    Keys { me, session, ltsm, k }
}

fn be(n: i64) -> String {
    B64.encode((n as u32).to_be_bytes())
}

/// A V2 message sealed with key `from_k` towards key `to_k` (both ours, so
/// our own mid is sender and recipient).
fn sealed(keys: &mut Keys, id: &str, from_k: usize, to_k: usize, text: &str) -> Message {
    let (sid, rid) = (keys.k[from_k].0, keys.k[to_k].0);
    let ch = keys
        .ltsm
        .create_channel(keys.k[from_k].2, &keys.k[to_k].1.clone())
        .unwrap();
    let pt = serde_json::to_vec(&json!({ "text": text })).unwrap();
    let ct = keys
        .ltsm
        .encrypt_v2(ch, &keys.me, &keys.me, sid as u32, rid as u32, 0, 7, &pt)
        .unwrap();
    keys.ltsm.release_channel(ch).unwrap();
    Message {
        id: id.into(),
        from: keys.me.clone(),
        to: keys.me.clone(),
        content_metadata: [("e2eeVersion".to_owned(), "2".to_owned())].into(),
        chunks: Some(vec![
            B64.encode(&ct[0..16]),
            B64.encode(&ct[28..]),
            B64.encode(&ct[16..28]),
            be(sid),
            be(rid),
        ]),
        ..Default::default()
    }
}

async fn client(server: &MockServer, keys: &Keys) -> Client {
    keys.session
        .clone()
        .into_client(Config {
            gateway: server.uri(),
            max_retries: 0,
            ..Config::default()
        })
        .unwrap()
}

fn kind(e: Error) -> E2eeErrorKind {
    e.e2ee_kind()
        .cloned()
        .unwrap_or_else(|| panic!("not an E2EE error: {e:?}"))
}

#[tokio::test]
#[ignore]
async fn decrypt_failures_are_typed_and_input_is_kept() {
    let mut k = keys();
    let server = MockServer::start().await;
    // Every public-key lookup fails like a retired key id does live.
    Mock::given(path("/api/talk/thrift/Talk/TalkService/getE2EEPublicKey"))
        .respond_with(talk_error(85))
        .mount(&server)
        .await;
    let c = client(&server, &k).await;

    // Our own keys are served locally even though the server refuses them.
    let good = sealed(&mut k, "m1", 0, 2, "works offline");
    assert_eq!(
        c.decrypt_message(&good).await.unwrap().text.as_deref(),
        Some("works offline")
    );

    // Missing own key.
    let mut m = sealed(&mut k, "m2", 0, 2, "x");
    m.chunks.as_mut().unwrap()[3] = be(123456);
    assert_eq!(
        kind(c.decrypt_message(&m).await.unwrap_err()),
        E2eeErrorKind::MissingOwnKey { key_id: 123456 }
    );

    // Tampered body: authentication failure, input unchanged.
    let mut m = sealed(&mut k, "m3", 0, 2, "tamper me");
    let mut body = B64.decode(&m.chunks.as_ref().unwrap()[1]).unwrap();
    body[0] ^= 1;
    m.chunks.as_mut().unwrap()[1] = B64.encode(&body);
    let before = m.clone();
    assert_eq!(
        kind(c.decrypt_message(&m).await.unwrap_err()),
        E2eeErrorKind::DecryptionFailed
    );
    assert_eq!(m, before);

    // Wrong content type in the AAD.
    let mut m = sealed(&mut k, "m4", 0, 2, "x");
    m.content_type = 15;
    assert_eq!(
        kind(c.decrypt_message(&m).await.unwrap_err()),
        E2eeErrorKind::DecryptionFailed
    );

    // Corrupted chunks / unsupported version.
    let mut m = sealed(&mut k, "m5", 0, 2, "x");
    m.chunks.as_mut().unwrap()[0] = "!!!".into();
    assert_eq!(
        kind(c.decrypt_message(&m).await.unwrap_err()),
        E2eeErrorKind::MalformedMessage
    );
    let mut m = sealed(&mut k, "m6", 0, 2, "x");
    m.chunks.as_mut().unwrap().truncate(2);
    assert_eq!(
        kind(c.decrypt_message(&m).await.unwrap_err()),
        E2eeErrorKind::MalformedMessage
    );
    let mut m = sealed(&mut k, "m7", 0, 2, "x");
    m.content_metadata.insert("e2eeVersion".into(), "3".into());
    assert_eq!(
        kind(c.decrypt_message(&m).await.unwrap_err()),
        E2eeErrorKind::UnsupportedVersion(3)
    );

    // A retired peer key (another mid) is reported as such.
    let mut m = sealed(&mut k, "m8", 0, 2, "x");
    m.from = "UxJrV_cK4MngglCvz0TksC8z_hZN3Zl77awCquia8P3s".into(); // any other user
    assert!(matches!(
        kind(c.decrypt_message(&m).await.unwrap_err()),
        E2eeErrorKind::PeerKeyUnavailable { .. }
    ));
}

#[tokio::test]
#[ignore]
async fn no_keys_is_reported() {
    let mut k = keys();
    let server = MockServer::start().await;
    let c = Client::with_tokens(
        Config {
            gateway: server.uri(),
            ..Config::default()
        },
        k.session.tokens.clone(),
    )
    .unwrap();
    let m = sealed(&mut k, "m1", 0, 2, "x");
    assert_eq!(
        kind(c.decrypt_message(&m).await.unwrap_err()),
        E2eeErrorKind::NoKeys
    );
}

#[tokio::test]
#[ignore]
async fn stream_decrypts_keeps_failures_and_drops_duplicates() {
    let mut k = keys();
    let good = sealed(&mut k, "s1", 1, 2, "hello stream");
    let mut bad = sealed(&mut k, "s2", 1, 2, "bad");
    bad.chunks.as_mut().unwrap()[2] = B64.encode([0u8; 12]);
    let op = |rev: i64, m: &Message| json!({"revision": rev.to_string(), "type": 26, "message": m});
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: {}\n\n",
        json!({"operations": [op(11, &good)]}),
        json!({"operations": [op(11, &good), op(12, &bad)]}), // 11 again: duplicate
        json!([op(
            13,
            &OutgoingMessage::text(&k.me, "plain")
                .to_json()
                .as_object()
                .map(|o| {
                    let mut m: Message = serde_json::from_value(Value::Object(o.clone())).unwrap();
                    m.id = "s3".into();
                    m
                })
                .unwrap()
        )]),
    );
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/operation/receive"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let c = client(&server, &k).await;
    let mut ops = c.operations(Some(10));
    assert!(ops.decrypt, "auto-decrypt is on when keys are loaded");
    let a = ops.next_operation().await.unwrap();
    assert_eq!(
        (a.revision, a.message.as_ref().unwrap().text.as_deref()),
        (11, Some("hello stream"))
    );
    assert!(a.e2ee_error.is_none());
    let b = ops.next_operation().await.unwrap();
    assert_eq!(b.revision, 12);
    assert_eq!(
        b.e2ee_error.as_ref().unwrap().kind,
        E2eeErrorKind::DecryptionFailed
    );
    assert!(b.message.unwrap().is_e2ee(), "sealed original kept");
    let c3 = ops.next_operation().await.unwrap();
    assert_eq!(
        (c3.revision, c3.message.unwrap().text.as_deref()),
        (13, Some("plain"))
    );
}

/// Mount negotiate (answers with `peer_keys[i]` for the i-th call, sticky at
/// the last) and sendMessage (`replies[i]` for the i-th sealed send, then OK).
async fn mount_send(
    server: &MockServer,
    k: &Keys,
    peer_keys: Vec<usize>,
    replies: Vec<i64>,
) -> (Arc<AtomicUsize>, Arc<Mutex<Vec<Value>>>) {
    let negotiations = Arc::new(AtomicUsize::new(0));
    let n = negotiations.clone();
    let offered: Vec<Value> = peer_keys
        .iter()
        .map(|&i| json!({"publicKey": {"keyId": k.k[i].0, "keyData": B64.encode(&k.k[i].1)}, "specVersion": 2, "allowedTypes": [0, 15]}))
        .collect();
    Mock::given(path("/api/talk/thrift/Talk/TalkService/negotiateE2EEPublicKey"))
        .respond_with(move |_: &Request| {
            let i = n.fetch_add(1, Ordering::SeqCst).min(offered.len() - 1);
            ok(offered[i].clone())
        })
        .mount(server)
        .await;
    let sent = Arc::new(Mutex::new(Vec::new()));
    let (s2, me) = (sent.clone(), k.me.clone());
    Mock::given(path("/api/talk/thrift/Talk/TalkService/sendMessage"))
        .respond_with(move |req: &Request| {
            let a: Value = serde_json::from_slice(&req.body).unwrap();
            let mut m = a[1].clone();
            if m.get("chunks").is_none() {
                return talk_error(82);
            }
            let i = {
                let mut s = s2.lock().unwrap();
                s.push(m.clone());
                s.len() - 1
            };
            if let Some(code) = replies.get(i) {
                return talk_error(*code);
            }
            m["id"] = json!(format!("sent{i}"));
            m["from"] = json!(me);
            ok(m)
        })
        .mount(server)
        .await;
    (negotiations, sent)
}

fn receiver_key_id(sent: &Value) -> i64 {
    let c = sent["chunks"].as_array().unwrap();
    let b = B64.decode(c[4].as_str().unwrap()).unwrap();
    b.iter().fold(0, |a, x| (a << 8) | *x as i64)
}

#[tokio::test]
#[ignore]
async fn peer_key_rotation_renegotiates_and_resends() {
    let k = keys();
    let server = MockServer::start().await;
    // First negotiation offers the oldest key; the server then says the
    // receiver key changed (84) and the next negotiation offers the middle key.
    let (neg, sent) = mount_send(&server, &k, vec![0, 1], vec![84]).await;
    let c = client(&server, &k).await;
    let m = c.send_text(&k.me, "after rotation").await.unwrap();
    let sent = sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(receiver_key_id(&sent[0]), k.k[0].0);
    assert_eq!(receiver_key_id(&sent[1]), k.k[1].0, "resent to the new key");
    assert_eq!(neg.load(Ordering::SeqCst), 2);
    assert_eq!(
        c.decrypt_message(&m).await.unwrap().text.as_deref(),
        Some("after rotation")
    );
}

#[tokio::test]
#[ignore]
async fn outdated_own_key_is_reported() {
    let k = keys();
    let server = MockServer::start().await;
    mount_send(&server, &k, vec![0], vec![83, 83, 83, 83]).await;
    let c = client(&server, &k).await;
    let e = c.send_text(&k.me, "x").await.unwrap_err();
    assert_eq!(kind(e), E2eeErrorKind::OwnKeyOutdated);
}

#[tokio::test]
#[ignore]
async fn key_update_operation_drops_cached_negotiation() {
    let k = keys();
    let server = MockServer::start().await;
    let (neg, _) = mount_send(&server, &k, vec![0, 1], vec![]).await;
    let c = client(&server, &k).await;
    c.send_text(&k.me, "one").await.unwrap();
    c.send_text(&k.me, "two").await.unwrap();
    assert_eq!(neg.load(Ordering::SeqCst), 1, "negotiation is cached");
    // NOTIFIED_E2EE_KEY_UPDATE mentioning the chat.
    c.note_operation(&Operation {
        r#type: 77,
        param1: Some(k.me.clone()),
        ..Default::default()
    });
    c.send_text(&k.me, "three").await.unwrap();
    assert_eq!(
        neg.load(Ordering::SeqCst),
        2,
        "re-negotiated after the key update"
    );
}

#[tokio::test]
#[ignore]
async fn state_survives_restart() {
    let mut k = keys();
    let server = MockServer::start().await;
    let (_, sent) = mount_send(&server, &k, vec![1], vec![]).await;
    let c = client(&server, &k).await;
    let echo = c.send_text(&k.me, "before restart").await.unwrap();

    let dir = std::env::temp_dir().join(format!("rune-restart-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("session.json");
    Session::from_client(&c).save(&file).unwrap();
    drop(c);

    let restored = Session::load(&file)
        .unwrap()
        .into_client(Config {
            gateway: server.uri(),
            ..Config::default()
        })
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(restored.e2ee_keys().len(), k.k.len());
    // Old sealed message (ours) and a message sealed to us still decrypt.
    assert_eq!(
        restored.decrypt_message(&echo).await.unwrap().text.as_deref(),
        Some("before restart")
    );
    let incoming = sealed(&mut k, "r1", 0, 2, "after restart");
    assert_eq!(
        restored.decrypt_message(&incoming).await.unwrap().text.as_deref(),
        Some("after restart")
    );
    // Sending still seals.
    restored.send_text(&k.me, "again").await.unwrap();
    assert_eq!(sent.lock().unwrap().len(), 2);
}
