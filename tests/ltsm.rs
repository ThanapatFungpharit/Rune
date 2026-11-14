//! Compatibility tests for the native `ltsm.wasm` host.
//!
//! `fixtures/hmac_vectors.json` was produced by the reference implementation
//! (OkLine's Node bridge running the same wasm inside LINE's own JS glue), see
//! `scripts/gen_hmac_vectors.js`.

use rune::ltsm::Ltsm;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vector {
    access_token: String,
    path: String,
    body: String,
    hmac: String,
}

fn vectors() -> Vec<Vector> {
    serde_json::from_str(include_str!("fixtures/hmac_vectors.json")).unwrap()
}

#[test]
fn hmac_matches_reference_bridge() {
    let mut l = Ltsm::new().unwrap();
    let vs = vectors();
    // Twice, interleaved, to exercise the derived-key cache switching tokens.
    for _ in 0..2 {
        for v in &vs {
            assert_eq!(
                l.hmac(&v.access_token, &v.path, &v.body).unwrap(),
                v.hmac,
                "{}",
                v.path
            );
        }
    }
    assert_eq!(l.live_js_values(), 0, "emval handles leaked");
}

#[test]
fn ecdh_channels_v1_roundtrip() {
    let mut l = Ltsm::new().unwrap();
    let a = l.generate_curve_key().unwrap();
    let b = l.generate_curve_key().unwrap();
    let pa = l.public_key(a).unwrap();
    let pb = l.public_key(b).unwrap();
    assert_eq!(pa.len(), 32);
    assert_ne!(pa, pb);

    let ab = l.create_channel(a, &pb).unwrap();
    let ba = l.create_channel(b, &pa).unwrap();
    let pt = br#"{"text":"hello sealed"}"#;
    let ct = l.encrypt_v1(ab, pt).unwrap();
    assert_ne!(&ct[..], &pt[..]);
    assert_eq!(l.decrypt_v1(ba, &ct).unwrap(), pt);

    // Tampering is detected.
    let mut bad = ct.clone();
    let last = bad.len() - 1;
    bad[last] ^= 1;
    assert!(l.decrypt_v1(ba, &bad).is_err());

    for h in [ab, ba] {
        l.release_channel(h).unwrap();
    }
    assert_eq!(l.live_js_values(), 0);
}

/// The module only lets keychain-unwrapped keys be exported (the reference
/// bridge behaves identically). This pins that behaviour so a change in a
/// future ltsm.wasm is noticed.
#[test]
fn synthetic_e2ee_keys_are_not_exportable() {
    let mut l = Ltsm::new().unwrap();
    let c = l.generate_curve_key().unwrap();
    let k = l.make_e2ee_key(c, 1, 1234).unwrap();
    assert_eq!(l.key_id(k).unwrap(), 1234);
    assert_eq!(l.key_version(k).unwrap(), 1);
    let e = l.export_e2ee_key(k).unwrap_err();
    assert!(e.0.contains("illegal operation"), "{e}");
}

/// Cross-implementation check: Node (LINE's own JS glue) encrypts, rust
/// decrypts, and vice versa. Needs `node` and the reference bridge:
///
/// ```sh
/// OKLINE_LTSM_DIR=/path/to/OkLine/okline/ltsm cargo test --test ltsm -- --ignored
/// ```
#[test]
#[ignore]
fn v1_interop_with_reference_bridge() {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as B64;
    use std::io::{BufRead, BufReader, Write};

    let dir = std::env::var("OKLINE_LTSM_DIR").expect("set OKLINE_LTSM_DIR");
    let mut child = std::process::Command::new("node")
        .arg(format!("{dir}/ltsm_bridge.js"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap(); // {"ready":true}
    let mut id = 0;
    let mut call = |req: serde_json::Value| -> serde_json::Value {
        id += 1;
        let mut req = req;
        req["id"] = id.into();
        writeln!(stdin, "{req}").unwrap();
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(v.get("error").is_none(), "{v}");
        v["result"].clone()
    };

    let mut l = Ltsm::new().unwrap();
    let mine = l.generate_curve_key().unwrap();
    let my_pub = l.public_key(mine).unwrap();
    let node_key = call(serde_json::json!({"op": "curvekey_generate"}));
    let node_pub = B64
        .decode(
            call(serde_json::json!({"op": "e2ee_public_key", "keyId": node_key}))
                .as_str()
                .unwrap(),
        )
        .unwrap();
    let node_ch = call(serde_json::json!({
        "op": "e2ee_create_channel_with_pubkey", "keyHandle": node_key, "peerPubKeyB64": B64.encode(&my_pub)
    }));
    let my_ch = l.create_channel(mine, &node_pub).unwrap();

    // node -> rust
    let pt = "สวัสดี from node".as_bytes();
    let ct = call(
        serde_json::json!({"op": "e2ee_encrypt_v1", "channelId": node_ch, "plaintextB64": B64.encode(pt)}),
    );
    let ct = B64.decode(ct.as_str().unwrap()).unwrap();
    assert_eq!(l.decrypt_v1(my_ch, &ct).unwrap(), pt);

    // rust -> node
    let ct = l.encrypt_v1(my_ch, b"hello from rust").unwrap();
    let back = call(
        serde_json::json!({"op": "e2ee_decrypt_v1", "channelId": node_ch, "ciphertextB64": B64.encode(&ct)}),
    );
    assert_eq!(B64.decode(back.as_str().unwrap()).unwrap(), b"hello from rust");

    let _ = child.kill();
    let _ = child.wait();
}

/// Letter Sealing V2 with *real* keys from a QR login and our real mid (the
/// module rejects made-up mids). Reads the session file; never prints keys.
///
/// ```sh
/// RUNE_SESSION=session.json cargo test --test ltsm real_keys -- --ignored
/// ```
#[test]
#[ignore]
fn real_keys_v2_roundtrip_and_reload() {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as B64;

    let path = std::env::var("RUNE_SESSION").expect("set RUNE_SESSION");
    let session = rune::Session::load(path).unwrap();
    let mid = session.tokens.mid.clone().unwrap();
    assert!(!session.e2ee_keys.is_empty(), "session has no E2EE keys");

    let mut l = Ltsm::new().unwrap();
    let mut handles = Vec::new();
    for k in &session.e2ee_keys {
        let h = l.load_e2ee_key(&B64.decode(&k.exported).unwrap()).unwrap();
        assert_eq!(l.key_id(h).unwrap(), k.key_id, "key id survives export/load");
        handles.push((k.key_id, h));
    }
    // Two of our own keys stand in for sender and receiver.
    let (sid, a) = handles[0];
    let (rid, b) = *handles.get(1).unwrap_or(&handles[0]);
    let pa = l.public_key(a).unwrap();
    let pb = l.public_key(b).unwrap();
    let ab = l.create_channel(a, &pb).unwrap();
    let ba = l.create_channel(b, &pa).unwrap();
    let pt = br#"{"text":"rune v2 self-test"}"#;
    let ct = l
        .encrypt_v2(ab, &mid, &mid, sid as u32, rid as u32, 0, 1, pt)
        .unwrap();
    assert_eq!(
        l.decrypt_v2(ba, &mid, &mid, sid as u32, rid as u32, 0, &ct)
            .unwrap(),
        pt
    );
    // Authenticated metadata: a different content type must fail.
    assert!(
        l.decrypt_v2(ba, &mid, &mid, sid as u32, rid as u32, 1, &ct)
            .is_err()
    );
    eprintln!("V2 ok with real keys {sid}/{rid}, ciphertext {} bytes", ct.len());
}
