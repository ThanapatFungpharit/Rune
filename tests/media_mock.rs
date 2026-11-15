//! Media upload/download against a mock gateway + OBS.

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use rune::media::decrypt_blob;
use rune::{Client, Config, MediaUpload, Message, Session, Tokens};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn ok(data: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"message": "OK", "data": data}))
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn params(req: &Request) -> Value {
    let h = req.headers.get("x-obs-params").unwrap().to_str().unwrap();
    serde_json::from_slice(&B64.decode(h).unwrap()).unwrap()
}

/// acquireEncryptedAccessToken: "TOKEN-1", then "TOKEN-2" after a renew.
async fn mount_token(server: &MockServer) {
    let n = Arc::new(Mutex::new(0));
    Mock::given(path(
        "/api/talk/thrift/Talk/TalkService/acquireEncryptedAccessToken",
    ))
    .respond_with(move |req: &Request| {
        assert_eq!(
            serde_json::from_slice::<Value>(&req.body).unwrap(),
            json!([2]),
            "OBS_GENERAL"
        );
        let mut n = n.lock().unwrap();
        *n += 1;
        ok(json!(format!("\u{1e}x\u{1e}TOKEN-{}\u{1f}meta", *n)))
    })
    .mount(server)
    .await;
}

#[tokio::test]
async fn plain_image_flow_v1_with_token_renewal() {
    let server = MockServer::start().await;
    mount_token(&server).await;
    Mock::given(path("/api/talk/thrift/Talk/TalkService/sendMessage"))
        .respond_with(|req: &Request| {
            let a: Value = serde_json::from_slice(&req.body).unwrap();
            let m = &a[1];
            assert_eq!(m["contentType"], 1);
            assert_eq!(m["hasContent"], true);
            assert!(m.get("chunks").is_none());
            ok(json!({"id": "777", "to": m["to"], "contentType": 1}))
        })
        .expect(1)
        .mount(&server)
        .await;
    let uploads = Arc::new(Mutex::new(Vec::<(String, Value, String, usize)>::new()));
    let up = uploads.clone();
    Mock::given(method("POST"))
        .and(path("/r/talk/m/777"))
        .respond_with(move |req: &Request| {
            let tok = req
                .headers
                .get("x-line-access")
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            // HTTP trims trailing whitespace from header values (the extension's value ends in a tab).
            assert_eq!(
                req.headers
                    .get("x-line-application")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .trim_end(),
                "CHROMEOS\t3.7.2\tChrome_OS"
            );
            assert!(req.headers.get("x-hmac").is_none(), "OBS requests are not signed");
            up.lock()
                .unwrap()
                .push((tok.clone(), params(req), req.url.path().into(), req.body.len()));
            // First token is rejected: the client must renew and retry.
            if tok == "TOKEN-1" {
                ResponseTemplate::new(401)
            } else {
                ResponseTemplate::new(201)
            }
        })
        .mount(&server)
        .await;

    let client = Client::with_tokens(
        Config {
            gateway: server.uri(),
            obs: server.uri(),
            ..Config::default()
        },
        Tokens {
            access_token: Some("A".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let sent = client
        .send_media("Cgroup", MediaUpload::image(png(2000, 1500), "photo.png"))
        .await
        .unwrap();
    assert_eq!(sent.id, "777");
    let ups = uploads.lock().unwrap().clone();
    assert_eq!(ups.len(), 2);
    assert_eq!(ups[1].0, "TOKEN-2");
    assert_eq!(
        ups[1].1,
        json!({"ver": "2.0", "name": "photo.jpg", "type": "image"})
    );
    assert!(ups[1].3 > 0);
}

#[tokio::test]
async fn plain_download_paths() {
    let server = MockServer::start().await;
    mount_token(&server).await;
    Mock::given(path("/r/talk/m/42/preview"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"P".to_vec()))
        .mount(&server)
        .await;
    Mock::given(path("/r/talk/m/42/original"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"O".to_vec()))
        .mount(&server)
        .await;
    Mock::given(path("/r/talk/m/43"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"F".to_vec()))
        .mount(&server)
        .await;
    let public = format!("{}/public/x.jpg", server.uri());
    Mock::given(path("/public/x.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"U".to_vec()))
        .mount(&server)
        .await;
    let client = Client::with_tokens(
        Config {
            gateway: server.uri(),
            obs: server.uri(),
            ..Config::default()
        },
        Tokens {
            access_token: Some("A".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let mut m = Message {
        id: "42".into(),
        content_type: 1,
        ..Default::default()
    };
    m.content_metadata
        .insert("MEDIA_CONTENT_INFO".into(), r#"{"category":"original"}"#.into());
    assert_eq!(client.download_media(&m, false).await.unwrap(), b"O");
    assert_eq!(client.download_media(&m, true).await.unwrap(), b"P");
    let f = Message {
        id: "43".into(),
        content_type: 14,
        ..Default::default()
    };
    assert_eq!(client.download_media(&f, false).await.unwrap(), b"F");
    let mut expired = f.clone();
    expired
        .content_metadata
        .insert("FILE_EXPIRE_TIMESTAMP".into(), "1000".into());
    assert!(matches!(
        client.download_media(&expired, false).await,
        Err(rune::Error::MediaUnavailable(_))
    ));
    let mut bot = Message {
        id: "44".into(),
        content_type: 1,
        ..Default::default()
    };
    bot.content_metadata.insert("DOWNLOAD_URL".into(), public);
    assert_eq!(client.download_media(&bot, false).await.unwrap(), b"U");
}

/// Parallel downloads share one OBS token fetch, and one renewal when that
/// token is refused (not one `acquireEncryptedAccessToken` per download).
#[tokio::test]
async fn parallel_downloads_share_the_obs_token() {
    let server = MockServer::start().await;
    mount_token(&server).await;
    Mock::given(path_regex(r"^/r/talk/m/\d+$"))
        .respond_with(|req: &Request| {
            let tok = req.headers.get("x-line-access").unwrap().to_str().unwrap();
            if tok == "TOKEN-1" {
                ResponseTemplate::new(401)
            } else {
                ResponseTemplate::new(200).set_body_bytes(req.url.path().as_bytes().to_vec())
            }
        })
        .mount(&server)
        .await;
    let client = Client::with_tokens(
        Config {
            gateway: server.uri(),
            obs: server.uri(),
            ..Config::default()
        },
        Tokens {
            access_token: Some("A".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let downloads = (0..8).map(|i| {
        let client = client.clone();
        tokio::spawn(async move {
            let m = Message {
                id: (100 + i).to_string(),
                content_type: 14,
                ..Default::default()
            };
            client.download_media(&m, false).await.unwrap()
        })
    });
    for (i, d) in downloads.enumerate() {
        assert_eq!(d.await.unwrap(), format!("/r/talk/m/{}", 100 + i).into_bytes());
    }
    let fetches = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path().ends_with("acquireEncryptedAccessToken"))
        .count();
    assert_eq!(fetches, 2, "one fetch, one renewal");
}

/// Media links in a message are the sender's choice: rune must not fetch
/// plain-http or private-network URLs, must not follow redirects off the OBS
/// host with credentials, must not hand OBS credentials to another host, and
/// must cap what it reads.
#[tokio::test]
async fn untrusted_media_links_and_oversized_bodies_are_refused() {
    let server = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    mount_token(&server).await;
    Mock::given(path("/x.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"secret".to_vec()))
        .mount(&elsewhere)
        .await;
    // A redirect from the OBS host to another one.
    Mock::given(path("/r/talk/m/50"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", format!("{}/x.jpg", elsewhere.uri())),
        )
        .mount(&server)
        .await;
    // A preview far larger than any picture.
    Mock::given(path("/r/talk/m/51/preview"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 21 * 1024 * 1024]))
        .mount(&server)
        .await;
    let client = Client::with_tokens(
        Config {
            gateway: server.uri(),
            obs: server.uri(),
            ..Config::default()
        },
        Tokens {
            access_token: Some("A".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let with_url = |id: &str, key: &str, url: String| {
        let mut m = Message {
            id: id.into(),
            content_type: 1,
            ..Default::default()
        };
        m.content_metadata.insert(key.into(), url);
        m
    };
    // Plain http on a host that is not OBS (and a private address at that).
    let err = client
        .download_media(
            &with_url("44", "DOWNLOAD_URL", format!("{}/x.jpg", elsewhere.uri())),
            false,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, rune::Error::MediaUnavailable(_)), "{err:?}");
    let err = client
        .download_media(
            &with_url("45", "PREVIEW_URL", "https://192.168.1.10/x.jpg".into()),
            true,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, rune::Error::MediaUnavailable(_)), "{err:?}");
    // IP literals are refused even when public (they would skip the resolver's check).
    for url in [
        "https://8.8.8.8/x.jpg",
        "https://[::ffff:169.254.169.254]/x",
        "https://0x7f.1/x",
    ] {
        let err = client
            .download_media(&with_url("46", "PREVIEW_URL", url.into()), true)
            .await
            .unwrap_err();
        assert!(matches!(err, rune::Error::MediaUnavailable(_)), "{url}: {err:?}");
    }
    // Redirected off the OBS host: refused.
    let plain = Message {
        id: "50".into(),
        content_type: 1,
        ..Default::default()
    };
    assert!(client.download_media(&plain, false).await.is_err());
    // OBS credentials never go to another host.
    assert!(
        client
            .download_obs(&format!("{}/x.jpg", elsewhere.uri()))
            .await
            .is_err()
    );
    assert!(client.download_obs("evil.example/r/x").await.is_err());
    assert!(
        elsewhere.received_requests().await.unwrap().is_empty(),
        "nothing may reach the other host"
    );
    // A 21 MiB preview is more than any picture: refused, not buffered.
    let big = Message {
        id: "51".into(),
        content_type: 1,
        ..Default::default()
    };
    let err = client.download_media(&big, true).await.unwrap_err();
    assert!(err.to_string().contains("limit"), "{err}");
}

/// Sealed (V2) media with real keys; our own mid stands in for the peer
/// (ltsm.wasm rejects made-up mids). No traffic reaches LINE.
///
/// ```sh
/// RUNE_SESSION=session.json cargo test --test media_mock -- --ignored
/// ```
#[tokio::test]
#[ignore]
async fn sealed_media_upload_and_download_round_trip() {
    let s = Session::load(std::env::var("RUNE_SESSION").expect("set RUNE_SESSION")).unwrap();
    let me = s.tokens.mid.clone().unwrap();
    let mut keys = s.e2ee_keys.clone();
    keys.sort_by_key(|k| k.key_id);
    let server = MockServer::start().await;
    mount_token(&server).await;
    // Peer = our oldest key.
    let mut l = rune::ltsm::Ltsm::new().unwrap();
    let pk = l.load_e2ee_key(&B64.decode(&keys[0].exported).unwrap()).unwrap();
    let peer_pub = l.public_key(pk).unwrap();
    Mock::given(path("/api/talk/thrift/Talk/TalkService/negotiateE2EEPublicKey"))
        .respond_with(ok(json!({"publicKey": {"keyId": keys[0].key_id, "keyData": B64.encode(&peer_pub)}, "specVersion": 2, "allowedTypes": [0, 1, 2, 3, 14, 15]})))
        .mount(&server)
        .await;
    Mock::given(path(
        "/api/talk/thrift/Talk/TalkService/determineMediaMessageFlow",
    ))
    .respond_with(ok(
        json!({"flowMap": {"1": 2, "2": 2, "3": 2, "14": 2}, "cacheTtlMillis": "21600000"}),
    ))
    .expect(1) // cached for the second send
    .mount(&server)
    .await;
    let store = Arc::new(Mutex::new(
        std::collections::HashMap::<String, (Value, Vec<u8>)>::new(),
    ));
    let st = store.clone();
    Mock::given(method("POST"))
        .and(path_regex(r"^/r/talk/em[ivaf]/"))
        .respond_with(move |req: &Request| {
            let p = req.url.path().to_owned();
            let mut resp = ResponseTemplate::new(201);
            let key = if p.contains("/reqid-") {
                let oid = "OIDXYZ".to_owned();
                resp = resp.insert_header("x-obs-oid", oid.as_str());
                p.rsplit_once('/').unwrap().0.to_owned() + "/" + &oid
            } else {
                p.clone()
            };
            st.lock().unwrap().insert(key, (params(req), req.body.clone()));
            resp
        })
        .mount(&server)
        .await;
    let sent_msgs = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (sm, me2) = (sent_msgs.clone(), me.clone());
    Mock::given(path("/api/talk/thrift/Talk/TalkService/sendMessage"))
        .respond_with(move |req: &Request| {
            let a: Value = serde_json::from_slice(&req.body).unwrap();
            let mut m = a[1].clone();
            assert!(m.get("chunks").is_some(), "media must be sent sealed");
            let meta = &m["contentMetadata"];
            assert!(
                meta.get("ENC_KM").is_none() && meta.get("FILE_NAME").is_none(),
                "key material must be inside the ciphertext: {meta}"
            );
            m["id"] = json!(format!("m{}", sm.lock().unwrap().len()));
            m["from"] = json!(me2);
            sm.lock().unwrap().push(m.clone());
            ok(m)
        })
        .mount(&server)
        .await;
    let stored = store.clone();
    Mock::given(method("GET"))
        .and(path_regex(r"^/r/talk/em[ivaf]/"))
        .respond_with(move |req: &Request| {
            assert!(
                req.headers.get("x-talk-meta").is_some(),
                "sealed downloads need X-Talk-Meta"
            );
            match stored.lock().unwrap().get(req.url.path()) {
                Some((_, body)) => ResponseTemplate::new(200).set_body_bytes(body.clone()),
                None => ResponseTemplate::new(404),
            }
        })
        .mount(&server)
        .await;

    let client = s
        .clone()
        .into_client(Config {
            gateway: server.uri(),
            obs: server.uri(),
            ..Config::default()
        })
        .unwrap();
    let img = png(1600, 1200);
    let sent: Message = client
        .send_media(&me, MediaUpload::image(img.clone(), "cat.png"))
        .await
        .unwrap();
    assert!(sent.is_e2ee());
    assert_eq!(sent.content_metadata["SID"], "emi");
    assert_eq!(sent.content_metadata["OID"], "OIDXYZ");
    let thumb: Value = serde_json::from_str(&sent.content_metadata["MEDIA_THUMB_INFO"]).unwrap();
    assert_eq!(thumb, json!({"width": 384, "height": 288}));
    {
        let st = store.lock().unwrap();
        let (p, _) = &st["/r/talk/emi/OIDXYZ"];
        assert_eq!(p["type"], "file");
        assert!(
            p["name"].as_str().unwrap().parse::<u128>().is_ok(),
            "name is a ms timestamp"
        );
        assert!(st.contains_key("/r/talk/emi/OIDXYZ__ud-preview"));
        assert!(
            !st.contains_key("/r/talk/emi/OIDXYZ__ud-hash"),
            "only video uploads chunk hashes"
        );
    }
    // Decrypt our sent message, then download + decrypt the blob.
    let opened = client.decrypt_message(&sent).await.unwrap();
    assert_eq!(opened.content_metadata["FILE_NAME"], "cat.jpg");
    let full = client.download_media(&sent, false).await.unwrap();
    let decoded = image::load_from_memory(&full).unwrap();
    assert_eq!(
        (decoded.width(), decoded.height()),
        (1478, 1108),
        "≤1280×1280 pixels, like the extension"
    );
    let pv = image::load_from_memory(&client.download_media(&sent, true).await.unwrap()).unwrap();
    assert_eq!((pv.width(), pv.height()), (384, 288));

    // Original: full resolution, metadata text chunk scrubbed.
    let crc = |b: &[u8]| {
        let mut c = 0xFFFF_FFFFu32;
        for &x in b {
            c ^= x as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    };
    let body = b"tEXtComment\0GPS 13.75,100.50";
    let mut text = ((body.len() - 4) as u32).to_be_bytes().to_vec();
    text.extend_from_slice(body);
    text.extend_from_slice(&crc(body).to_be_bytes());
    let tagged = [&img[..33], &text, &img[33..]].concat();
    let so = client
        .send_media(
            &me,
            MediaUpload {
                original: true,
                ..MediaUpload::image(tagged, "cat.png")
            },
        )
        .await
        .unwrap();
    let opened = client.decrypt_message(&so).await.unwrap();
    assert_eq!(opened.content_metadata["FILE_NAME"], "cat.png");
    let full = client.download_media(&so, false).await.unwrap();
    assert!(!String::from_utf8_lossy(&full).contains("GPS"));
    let decoded = image::load_from_memory(&full).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (1600, 1200));

    // Video: chunk hashes are uploaded and the blob uses the chunked tag.
    let video: Vec<u8> = (0..400_000u32).map(|i| (i % 200) as u8).collect();
    let sv = client
        .send_media(&me, MediaUpload::video(video.clone(), "clip.mp4", 3000))
        .await
        .unwrap();
    assert_eq!(sv.content_metadata["SID"], "emv");
    assert_eq!(sv.content_metadata["DURATION"], "3000");
    assert!(store.lock().unwrap().contains_key("/r/talk/emv/OIDXYZ__ud-hash"));
    assert_eq!(client.download_media(&sv, false).await.unwrap(), video);
    // The raw stored blob is the extension's format: decryptable with ENC_KM.
    let km = B64
        .decode(&client.decrypt_message(&sv).await.unwrap().content_metadata["ENC_KM"])
        .unwrap();
    let raw = store.lock().unwrap()["/r/talk/emv/OIDXYZ"].1.clone();
    assert_eq!(decrypt_blob(&km, &raw, true).unwrap(), video);
}
