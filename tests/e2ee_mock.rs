//! Letter Sealing flows against a mock gateway, using *real* keychain keys
//! from a session file. One of our keys plays "us", another plays the peer.
//! No traffic reaches LINE.
//!
//! `ltsm.wasm` decodes the (authenticated) new-style mids it is given and
//! fails with "data authentication failure" for made-up ones, so the peer is
//! our own mid (a note-to-self chat) and the group is a real group mid.
//!
//! ```sh
//! RUNE_SESSION=session.json RUNE_TEST_GROUP=<a group mid you are in> \
//!   cargo test --test e2ee_mock -- --ignored
//! ```

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use rune::ltsm::{KeyHandle, Ltsm};
use rune::{Config, Message, Session, Tokens};
use serde_json::{Value, json};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn ok(data: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"message": "OK", "data": data}))
}

fn talk_error(code: i64) -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(json!({"code": 10051, "message": "RESPONSE_ERROR",
        "data": {"name": "TalkException", "code": code, "reason": "mock"}}))
}

struct Fixture {
    session: Session,
    me: String,
    /// The peer's mid (our own, see the module docs).
    peer_mid: String,
    /// (key id, public key) of our latest key and of the key playing the peer.
    mine: (i64, Vec<u8>),
    peer: (i64, Vec<u8>),
    /// A separate module holding the peer's private key.
    peer_ltsm: Ltsm,
    peer_key: KeyHandle,
}

fn fixture() -> Fixture {
    let session = Session::load(std::env::var("RUNE_SESSION").expect("set RUNE_SESSION")).unwrap();
    let me = session.tokens.mid.clone().unwrap();
    let mut keys = session.e2ee_keys.clone();
    keys.sort_by_key(|k| k.key_id);
    assert!(keys.len() >= 2, "need two keys in the session");
    let (latest, other) = (keys.last().unwrap().clone(), keys[0].clone());
    let mut l = Ltsm::new().unwrap();
    let lh = l.load_e2ee_key(&B64.decode(&latest.exported).unwrap()).unwrap();
    let mut peer_ltsm = Ltsm::new().unwrap();
    let peer_key = peer_ltsm
        .load_e2ee_key(&B64.decode(&other.exported).unwrap())
        .unwrap();
    let peer_pub = peer_ltsm.public_key(peer_key).unwrap();
    Fixture {
        mine: (latest.key_id as i64, l.public_key(lh).unwrap()),
        peer: (other.key_id as i64, peer_pub),
        session: Session {
            tokens: Tokens {
                access_token: Some("A".into()),
                mid: Some(me.clone()),
                ..Default::default()
            },
            e2ee_keys: session.e2ee_keys,
        },
        peer_mid: me.clone(),
        me,
        peer_ltsm,
        peer_key,
    }
}

/// Mount getE2EEPublicKey / negotiate / sendMessage (plain -> 82, sealed -> echo).
async fn mount_common(
    server: &MockServer,
    f: &Fixture,
    sent: Arc<Mutex<Vec<Value>>>,
    plain_attempts: Arc<Mutex<u32>>,
) {
    let keys = json!({
        f.me.clone(): {f.mine.0.to_string(): B64.encode(&f.mine.1), f.peer.0.to_string(): B64.encode(&f.peer.1)},
    });
    Mock::given(path("/api/talk/thrift/Talk/TalkService/getE2EEPublicKey"))
        .respond_with(move |req: &Request| {
            let a: Value = serde_json::from_slice(&req.body).unwrap();
            let (mid, id) = (a[0].as_str().unwrap(), a[2].as_i64().unwrap());
            match keys[mid][id.to_string()].as_str() {
                Some(k) => ok(json!({"keyId": id, "keyData": k, "version": 1})),
                None => talk_error(5),
            }
        })
        .mount(server)
        .await;
    let peer = json!({"publicKey": {"keyId": f.peer.0, "keyData": B64.encode(&f.peer.1)}, "specVersion": 2, "allowedTypes": [0, 15]});
    Mock::given(path("/api/talk/thrift/Talk/TalkService/negotiateE2EEPublicKey"))
        .respond_with(ok(peer))
        .mount(server)
        .await;
    let me = f.me.clone();
    Mock::given(path("/api/talk/thrift/Talk/TalkService/sendMessage"))
        .respond_with(move |req: &Request| {
            let a: Value = serde_json::from_slice(&req.body).unwrap();
            let mut m = a[1].clone();
            if m.get("chunks").is_none() {
                *plain_attempts.lock().unwrap() += 1;
                return talk_error(82);
            }
            assert!(m.get("text").is_none() && m.get("from").is_none(), "{m}");
            sent.lock().unwrap().push(m.clone());
            m["id"] = json!(format!("900{}", sent.lock().unwrap().len()));
            m["from"] = json!(me);
            m["createdTime"] = json!("1794577600000");
            ok(m)
        })
        .mount(server)
        .await;
}

#[tokio::test]
#[ignore]
async fn one_to_one_seal_fallback_and_decrypt() {
    let mut f = fixture();
    let server = MockServer::start().await;
    let sent = Arc::new(Mutex::new(Vec::new()));
    let plain = Arc::new(Mutex::new(0));
    mount_common(&server, &f, sent.clone(), plain.clone()).await;
    let client = f
        .session
        .clone()
        .into_client(Config {
            gateway: server.uri(),
            ..Config::default()
        })
        .unwrap();
    assert!(client.e2ee_ready());

    // Plain send -> 82 -> sealed resend; the next send is sealed up front.
    let echo = client.send_text(&f.peer_mid, "hello sealed สวัสดี").await.unwrap();
    client.send_text(&f.peer_mid, "second").await.unwrap();
    assert_eq!(*plain.lock().unwrap(), 1);
    let sealed = sent.lock().unwrap().clone();
    assert_eq!(sealed.len(), 2);
    assert_eq!(sealed[0]["contentMetadata"]["e2eeVersion"], "2");
    assert_eq!(sealed[0]["chunks"].as_array().unwrap().len(), 5);

    // Our own sent message decrypts (ECDH with the recipient's key).
    assert!(echo.is_e2ee());
    let opened = client.decrypt_message(&echo).await.unwrap();
    assert_eq!(opened.text.as_deref(), Some("hello sealed สวัสดี"));

    // The peer can open it with *its* private key.
    let chunks: Vec<String> = serde_json::from_value(sealed[1]["chunks"].clone()).unwrap();
    let ct = [
        B64.decode(&chunks[0]).unwrap(),
        B64.decode(&chunks[2]).unwrap(),
        B64.decode(&chunks[1]).unwrap(),
    ]
    .concat();
    let ch = f.peer_ltsm.create_channel(f.peer_key, &f.mine.1).unwrap();
    let pt = f
        .peer_ltsm
        .decrypt_v2(ch, &f.peer_mid, &f.me, f.mine.0 as u32, f.peer.0 as u32, 0, &ct)
        .unwrap();
    assert_eq!(pt, br#"{"text":"second"}"#);

    // A message sealed by the peer to us decrypts.
    let ct = f
        .peer_ltsm
        .encrypt_v2(
            ch,
            &f.me,
            &f.peer_mid,
            f.peer.0 as u32,
            f.mine.0 as u32,
            0,
            1,
            br#"{"text":"from peer"}"#,
        )
        .unwrap();
    let be = |n: i64| B64.encode((n as u32).to_be_bytes());
    let incoming = Message {
        id: "1".into(),
        from: f.peer_mid.clone(),
        to: f.me.clone(),
        content_metadata: [("e2eeVersion".to_owned(), "2".to_owned())].into(),
        chunks: Some(vec![
            B64.encode(&ct[0..16]),
            B64.encode(&ct[28..]),
            B64.encode(&ct[16..28]),
            be(f.peer.0),
            be(f.mine.0),
        ]),
        ..Default::default()
    };
    assert_eq!(
        client.decrypt_message(&incoming).await.unwrap().text.as_deref(),
        Some("from peer")
    );
}

#[tokio::test]
#[ignore]
async fn group_key_registration_seal_and_member_decrypt() {
    let mut f = fixture();
    let group = std::env::var("RUNE_TEST_GROUP").expect("set RUNE_TEST_GROUP to a real group mid");
    let group = group.as_str();
    let server = MockServer::start().await;
    let sent = Arc::new(Mutex::new(Vec::new()));
    mount_common(&server, &f, sent.clone(), Arc::new(Mutex::new(0))).await;
    Mock::given(path(
        "/api/talk/thrift/Talk/TalkService/getLastE2EEGroupSharedKey",
    ))
    .respond_with(talk_error(5)) // NOT_FOUND: no group key yet
    .mount(&server)
    .await;
    // Two "members" sharing our mid would collide in a map, so the second
    // member gets a distinct key; the mock maps it back below.
    let members = json!({
        f.me.clone(): {"keyId": f.mine.0, "keyData": B64.encode(&f.mine.1)},
        "member-2": {"keyId": f.peer.0, "keyData": B64.encode(&f.peer.1)},
    });
    Mock::given(path("/api/talk/thrift/Talk/TalkService/getLastE2EEPublicKeys"))
        .respond_with(ok(members))
        .mount(&server)
        .await;
    let registered = Arc::new(Mutex::new(Value::Null));
    let (reg, me, my_kid, want_group) = (registered.clone(), f.me.clone(), f.mine.0, group.to_owned());
    Mock::given(path("/api/talk/thrift/Talk/TalkService/registerE2EEGroupKey"))
        .respond_with(move |req: &Request| {
            let a: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(a[0], 1);
            assert_eq!(a[1], want_group.as_str());
            *reg.lock().unwrap() = a.clone();
            let i = a[2].as_array().unwrap().iter().position(|m| m == &json!(me)).unwrap();
            ok(json!({"groupKeyId": 77, "creator": me, "creatorKeyId": my_kid, "receiver": me,
                      "receiverKeyId": my_kid, "encryptedSharedKey": a[4][i], "specVersion": 2, "allowedTypes": []}))
        })
        .expect(1)
        .mount(&server)
        .await;

    let client = f
        .session
        .clone()
        .into_client(Config {
            gateway: server.uri(),
            ..Config::default()
        })
        .unwrap();
    let echo = client.send_text(group, "hi group").await.unwrap();
    assert_eq!(
        client.decrypt_message(&echo).await.unwrap().text.as_deref(),
        Some("hi group")
    );

    // The other member unwraps its copy of the group key and reads the message.
    let reg = registered.lock().unwrap().clone();
    let i = reg[2]
        .as_array()
        .unwrap()
        .iter()
        .position(|m| m == "member-2")
        .unwrap();
    assert_eq!(reg[3][i], f.peer.0);
    let wrapped = B64.decode(reg[4][i].as_str().unwrap()).unwrap();
    let unwrap_ch = f.peer_ltsm.create_channel(f.peer_key, &f.mine.1).unwrap(); // ECDH(peer, creator)
    let gk = f.peer_ltsm.unwrap_group_shared_key(unwrap_ch, &wrapped).unwrap();
    let chunks: Vec<String> = serde_json::from_value(sent.lock().unwrap()[0]["chunks"].clone()).unwrap();
    let ct = [
        B64.decode(&chunks[0]).unwrap(),
        B64.decode(&chunks[2]).unwrap(),
        B64.decode(&chunks[1]).unwrap(),
    ]
    .concat();
    let ch = f.peer_ltsm.create_channel(gk, &f.mine.1).unwrap(); // ECDH(group key, sender)
    let pt = f
        .peer_ltsm
        .decrypt_v2(ch, group, &f.me, f.mine.0 as u32, 77, 0, &ct)
        .unwrap();
    assert_eq!(pt, br#"{"text":"hi group"}"#);
}
