//! Client behaviour against a mock gateway (no real LINE traffic).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rune::ltsm::Ltsm;
use rune::{Client, Config, Error, QrLoginEvent, QrLoginOptions, Tokens};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn ok(data: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"message": "OK", "data": data}))
}

fn config(server: &MockServer) -> Config {
    Config {
        gateway: server.uri(),
        max_retries: 1,
        ..Config::default()
    }
}

fn logged_in(server: &MockServer) -> Client {
    let tokens = Tokens {
        access_token: Some("ACCESS-1".into()),
        refresh_token: Some("REFRESH-1".into()),
        mid: Some("u00000000000000000000000000000001".into()),
        ..Default::default()
    };
    Client::with_tokens(config(server), tokens).unwrap()
}

/// The expected X-Hmac, from the (reference-validated) secure module.
fn hmac(token: &str, path: &str, body: &str) -> String {
    Ltsm::new().unwrap().hmac(token, path, body).unwrap()
}

#[tokio::test]
async fn thrift_call_wire_format() {
    let server = MockServer::start().await;
    let p = "/api/talk/thrift/Talk/TalkService/getProfile";
    Mock::given(method("POST"))
        .and(path(p))
        .and(header("x-line-access", "ACCESS-1"))
        .and(header("x-line-chrome-version", "3.7.2"))
        .and(header("x-lal", "en_US"))
        .and(header("content-type", "application/json"))
        .and(header("x-hmac", hmac("ACCESS-1", p, "[2]").as_str()))
        .and(body_json(json!([2])))
        .respond_with(ok(
            json!({"mid": "u1", "displayName": "Me", "statusMessage": null, "regionCode": "TH"}),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let client = logged_in(&server);
    let p = client.profile().await.unwrap();
    assert_eq!(p.display_name, "Me");
    assert_eq!(p.status_message, "");
    assert_eq!(p.region_code.as_deref(), Some("TH"));
}

#[tokio::test]
async fn send_text_body_is_exact_and_signed() {
    let server = MockServer::start().await;
    let p = "/api/talk/thrift/Talk/TalkService/sendMessage";
    Mock::given(method("POST"))
        .and(path(p))
        .respond_with(|req: &Request| {
            // The signature must cover exactly the transmitted bytes.
            let body = std::str::from_utf8(&req.body).unwrap();
            let sig = req.headers.get("x-hmac").unwrap().to_str().unwrap();
            assert_eq!(sig, hmac("ACCESS-1", "/api/talk/thrift/Talk/TalkService/sendMessage", body));
            let args: Value = serde_json::from_str(body).unwrap();
            assert!(args[0].is_i64(), "reqSeq");
            assert_eq!(args[1]["to"], "C123");
            assert_eq!(args[1]["toType"], 2);
            assert_eq!(args[1]["text"], "สวัสดี 👋");
            ok(json!({"id": "9001", "to": "C123", "toType": 2, "contentType": 0, "createdTime": "1700000000000"}))
        })
        .expect(1)
        .mount(&server)
        .await;

    let m = logged_in(&server).send_text("C123", "สวัสดี 👋").await.unwrap();
    assert_eq!(m.id, "9001");
    assert_eq!(m.created_time, Some(1_700_000_000_000));
}

#[tokio::test]
async fn talk_exception_is_mapped() {
    let server = MockServer::start().await;
    Mock::given(path("/api/talk/thrift/Talk/TalkService/sendMessage"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": 10051, "message": "RESPONSE_ERROR",
            "data": {"name": "TalkException", "code": 82, "reason": "can not send using plain mode"}
        })))
        .mount(&server)
        .await;
    let err = logged_in(&server).send_text("u1", "x").await.unwrap_err();
    let api = err.api().expect("api error");
    assert!(api.requires_e2ee());
    assert_eq!(api.reason.as_deref(), Some("can not send using plain mode"));
}

#[tokio::test]
async fn must_refresh_token_refreshes_and_replays() {
    let server = MockServer::start().await;
    let p = "/api/talk/thrift/Talk/TalkService/getServerTime";
    // Old token -> TalkException 119.
    Mock::given(path(p))
        .and(header("x-line-access", "ACCESS-1"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"code": 10051, "data": {"code": 119}})))
        .expect(1)
        .mount(&server)
        .await;
    // Refresh carries the old access token + refresh token.
    Mock::given(path("/api/auth/tokenRefresh"))
        .and(header("x-line-access", "ACCESS-1"))
        .and(header(
            "x-hmac",
            hmac(
                "ACCESS-1",
                "/api/auth/tokenRefresh",
                r#"{"refreshToken":"REFRESH-1"}"#,
            )
            .as_str(),
        ))
        .and(body_json(json!({"refreshToken": "REFRESH-1"})))
        .respond_with(ok(json!({"tokenV3IssueResult": {
            "accessToken": "ACCESS-2", "refreshToken": "REFRESH-2",
            "tokenIssueTimeEpochSec": "1700000000", "durationUntilRefreshInSec": "86400"}})))
        .expect(1)
        .mount(&server)
        .await;
    // Replay is re-signed with the new token.
    Mock::given(path(p))
        .and(header("x-line-access", "ACCESS-2"))
        .and(header("x-hmac", hmac("ACCESS-2", p, "[]").as_str()))
        .respond_with(ok(json!("1700000000123")))
        .expect(1)
        .mount(&server)
        .await;

    let client = logged_in(&server);
    let mut updates = client.subscribe_tokens();
    assert_eq!(client.server_time().await.unwrap(), 1_700_000_000_123);
    let t = client.tokens();
    assert_eq!(t.access_token.as_deref(), Some("ACCESS-2"));
    assert_eq!(t.refresh_token.as_deref(), Some("REFRESH-2"));
    assert_eq!(t.refresh_due_epoch_sec(), Some(1_700_086_400));
    assert!(updates.has_changed().unwrap());
    assert_eq!(
        updates.borrow_and_update().access_token.as_deref(),
        Some("ACCESS-2")
    );
}

#[tokio::test]
async fn rejected_refresh_requires_relogin() {
    let server = MockServer::start().await;
    Mock::given(path("/api/talk/thrift/Talk/TalkService/getServerTime"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"code": 10051, "data": {"code": 119}})))
        .mount(&server)
        .await;
    Mock::given(path("/api/auth/tokenRefresh"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"code": 10201, "message": "AUTH_INVALID_REQUEST"})),
        )
        .mount(&server)
        .await;
    let err = logged_in(&server).server_time().await.unwrap_err();
    assert!(matches!(err, Error::Reauth(_)), "{err:?}");
}

#[tokio::test]
async fn retries_unknown_error_and_5xx() {
    struct Flaky(AtomicUsize);
    impl Respond for Flaky {
        fn respond(&self, _: &Request) -> ResponseTemplate {
            match self.0.fetch_add(1, Ordering::SeqCst) {
                0 => ResponseTemplate::new(503),
                1 => ResponseTemplate::new(200)
                    .set_body_json(json!({"code": 99999, "message": "UNKNOWN_ERROR"})),
                _ => ok(json!(42)),
            }
        }
    }
    let server = MockServer::start().await;
    Mock::given(path("/api/talk/thrift/Talk/TalkService/getLastOpRevision"))
        .respond_with(Flaky(AtomicUsize::new(0)))
        .mount(&server)
        .await;
    let client = Client::with_tokens(
        Config {
            gateway: server.uri(),
            max_retries: 2,
            ..Config::default()
        },
        Tokens {
            access_token: Some("A".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(client.last_op_revision().await.unwrap(), 42);
}

#[tokio::test]
async fn auth_kickout_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(path("/api/talk/thrift/Talk/TalkService/getProfile"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(
                json!({"code": 10051, "data": {"code": 8, "reason": "NOT_AUTHORIZED_DEVICE"}}),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    let err = logged_in(&server).profile().await.unwrap_err();
    assert!(err.api().unwrap().is_auth());
}

#[tokio::test]
async fn not_logged_in_fails_fast() {
    let server = MockServer::start().await;
    let client = Client::new(config(&server)).unwrap();
    assert!(matches!(client.profile().await, Err(Error::LoginRequired)));
}

#[tokio::test]
async fn contacts_are_batched_and_unwrapped() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    Mock::given(path("/api/talk/thrift/Talk/TalkService/getContactsV2"))
        .respond_with(move |req: &Request| {
            c2.fetch_add(1, Ordering::SeqCst);
            let args: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(args[1], 4); // FULL_SYNC
            let mids = args[0]["targetUserMids"].as_array().unwrap();
            assert!(mids.len() <= 100);
            let contacts: serde_json::Map<String, Value> = mids
                .iter()
                .map(|m| {
                    (
                        m.as_str().unwrap().to_owned(),
                        json!({"contact": {"mid": m, "displayName": "n"}}),
                    )
                })
                .collect();
            ok(json!({"contacts": contacts}))
        })
        .mount(&server)
        .await;
    let mids: Vec<String> = (0..150).map(|i| format!("u{i:032}")).collect();
    let got = logged_in(&server).contacts(&mids).await.unwrap();
    assert_eq!(got.len(), 150);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn operation_stream_resumes_from_cursor_and_dedupes() {
    let server = MockServer::start().await;
    Mock::given(path("/api/talk/thrift/Talk/TalkService/getLastOpRevision"))
        .respond_with(ok(json!("100")))
        .mount(&server)
        .await;
    let body = concat!(
        ": hello\n\n",
        "event: ping\ndata: {}\n\n",
        "data: {\"operations\":[{\"revision\":\"99\",\"type\":26},{\"revision\":\"101\",\"type\":26,",
        "\"message\":{\"id\":\"1\",\"from\":\"uA\",\"to\":\"uB\",\"toType\":0,\"text\":\"hi\",\"contentType\":0}}]}\n\n",
        "event: fullSync\ndata: {\"nextRevision\":\"150\"}\n\n",
        "data: [{\"revision\":\"151\",\"type\":55,\"param1\":\"c1\"}]\n\n",
    );
    Mock::given(method("GET"))
        .and(path("/api/operation/receive"))
        .respond_with(move |req: &Request| {
            let q = req.url.query().unwrap();
            assert!(
                q.starts_with("version=3.7.2&language=en_US&lastPartialFullSyncs=%7B%7D&localRev=100"),
                "{q}"
            );
            let signed = format!("/api/operation/receive?{q}");
            assert_eq!(
                req.headers.get("x-hmac").unwrap().to_str().unwrap(),
                hmac("ACCESS-1", &signed, "")
            );
            assert_eq!(req.headers.get("accept").unwrap(), "text/event-stream");
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body)
        })
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let client = logged_in(&server);
    let mut ops = client.operations(None);
    let first = ops.next_operation().await.unwrap();
    assert_eq!(first.revision, 101, "revision 99 is behind the cursor");
    assert_eq!(first.message.unwrap().text.as_deref(), Some("hi"));
    let second = ops.next_operation().await.unwrap();
    assert_eq!(second.revision, 151);
    assert_eq!(second.param1.as_deref(), Some("c1"));
    assert_eq!(ops.local_rev, Some(151));
}

#[tokio::test]
async fn qr_login_full_flow_with_pin() {
    let server = MockServer::start().await;
    let svc = "/api/talk/thrift/LoginQrCode/SecondaryQrCodeLoginService";
    let notice = "/api/talk/thrift/LoginQrCode/SecondaryQrCodeLoginPermitNoticeService";
    Mock::given(path(format!("{svc}/createSession")))
        .and(body_json(json!([{}])))
        .respond_with(ok(json!({"authSessionId": "SID"})))
        .mount(&server)
        .await;
    Mock::given(path(format!("{svc}/createQrCode")))
        .and(body_json(json!([{"authSessionId": "SID"}])))
        .respond_with(ok(json!({"callbackUrl": "https://line.me/R/au/lgn/sq/SID", "longPollingIntervalSec": 1, "longPollingMaxCount": 3})))
        .mount(&server)
        .await;
    // First poll window elapses (410 inside a 10052 envelope), second succeeds.
    Mock::given(path(format!("{notice}/checkQrCodeVerified")))
        .and(header("x-line-session-id", "SID"))
        .and(header("x-lst", "1000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"code": 10052, "message": "RESPONSE_HTTP_ERROR", "data": {"statusCode": 410}}),
        ))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path(format!("{notice}/checkQrCodeVerified")))
        .respond_with(ok(json!({})))
        .mount(&server)
        .await;
    Mock::given(path(format!("{svc}/verifyCertificate")))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"code": 10051, "data": {"code": 43}})))
        .mount(&server)
        .await;
    Mock::given(path(format!("{svc}/createPinCode")))
        .respond_with(ok(json!({"pinCode": "123456"})))
        .mount(&server)
        .await;
    Mock::given(path(format!("{notice}/checkPinCodeVerified")))
        .and(header("x-lst", "110000"))
        .respond_with(ok(json!({})))
        .mount(&server)
        .await;
    Mock::given(path(format!("{svc}/qrCodeLoginV2")))
        .and(body_json(json!([{"systemName": "CHROMEOS", "modelName": "CHROME", "autoLoginIsRequired": false, "authSessionId": "SID"}])))
        .respond_with(ok(json!({
            "mid": "uME", "certificate": "CERT",
            "tokenV3IssueResult": {"accessToken": "A1", "refreshToken": "R1", "tokenIssueTimeEpochSec": 1, "durationUntilRefreshInSec": 2}
        })))
        .mount(&server)
        .await;

    let client = Client::new(config(&server)).unwrap();
    let mut events = Vec::new();
    let res = client
        .qr_login(QrLoginOptions::default(), |e| events.push(e))
        .await
        .unwrap();

    assert_eq!(events.len(), 2);
    let QrLoginEvent::QrUrl(url) = &events[0] else {
        panic!("{events:?}")
    };
    assert!(
        url.starts_with("https://line.me/R/au/lgn/sq/SID?secret="),
        "{url}"
    );
    assert!(url.ends_with("&e2eeVersion=1"));
    assert_eq!(events[1], QrLoginEvent::Pin("123456".into()));

    assert_eq!(res.mid.as_deref(), Some("uME"));
    assert!(res.e2ee_keys.is_empty());
    assert!(res.e2ee_error.is_some());
    let t = client.tokens();
    assert_eq!(t.access_token.as_deref(), Some("A1"));
    assert_eq!(t.certificate.as_deref(), Some("CERT"));
    assert_eq!(t.mid.as_deref(), Some("uME"));
    // Pre-login requests are signed with the empty token and carry no X-Line-Access.
    for r in server.received_requests().await.unwrap() {
        if r.url.path().contains("LoginQrCode") {
            assert!(r.headers.get("x-line-access").is_none());
            let body = std::str::from_utf8(&r.body).unwrap();
            assert_eq!(
                r.headers.get("x-hmac").unwrap().to_str().unwrap(),
                hmac("", r.url.path(), body)
            );
        }
    }
}

#[tokio::test]
async fn invitations_list_accept_and_decline() {
    let server = MockServer::start().await;
    let talk = |m: &str| format!("/api/talk/thrift/Talk/TalkService/{m}");
    Mock::given(method("POST"))
        .and(path(talk("getAllChatMids")))
        .and(body_json(
            json!([{ "withMemberChats": true, "withInvitedChats": true }, 4]),
        ))
        .respond_with(ok(json!({"memberChatMids": ["C1"], "invitedChatMids": ["C2"]})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(talk("getChats")))
        .respond_with(|req: &Request| {
            let args: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(args[0]["chatMids"], json!(["C2"]), "only invited chats are fetched");
            ok(json!({"chats": [{"chatMid": "C2", "chatName": "Invited", "type": 0,
                                 "createdTime": "1700000000000",
                                 "extra": {"groupExtra": {"memberMids": {"u9": 1}, "inviteeMids": {"u1": 2}}}}]}))
        })
        .mount(&server)
        .await;
    for m in ["acceptChatInvitation", "rejectChatInvitation"] {
        Mock::given(method("POST"))
            .and(path(talk(m)))
            .respond_with(|req: &Request| {
                let args: Value = serde_json::from_slice(&req.body).unwrap();
                assert!(args[0]["reqSeq"].is_i64());
                assert_eq!(args[0]["chatMid"], "C2");
                ok(json!({}))
            })
            .expect(1)
            .mount(&server)
            .await;
    }

    let client = logged_in(&server);
    let inv = client.invited_chats().await.unwrap();
    assert_eq!(inv.len(), 1);
    assert_eq!(
        (inv[0].chat_mid.as_str(), inv[0].chat_name.as_str()),
        ("C2", "Invited")
    );
    assert_eq!(inv[0].created_time(), Some(1_700_000_000_000));
    client.accept_chat_invitation("C2").await.unwrap();
    client.reject_chat_invitation("C2").await.unwrap();
}

fn now_sec() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
async fn token_refresher_renews_ahead_of_time() {
    let server = MockServer::start().await;
    let issued = now_sec();
    Mock::given(path("/api/auth/tokenRefresh"))
        .and(body_json(json!({"refreshToken": "REFRESH-1"})))
        .respond_with(ok(json!({"tokenV3IssueResult": {
            "accessToken": "ACCESS-2", "refreshToken": "REFRESH-2",
            "tokenIssueTimeEpochSec": issued.to_string(), "durationUntilRefreshInSec": "86400"}})))
        .expect(1)
        .mount(&server)
        .await;
    // Due an hour ago: renewed at once, no request needed to trigger it.
    let tokens = Tokens {
        access_token: Some("ACCESS-1".into()),
        refresh_token: Some("REFRESH-1".into()),
        token_issue_time_epoch_sec: Some(issued - 7200),
        duration_until_refresh_in_sec: Some(3600),
        ..Default::default()
    };
    let client = Client::with_tokens(config(&server), tokens).unwrap();
    let task = client.spawn_token_refresher();
    let mut rx = client.subscribe_tokens();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while client.tokens().access_token.as_deref() != Some("ACCESS-2") {
            rx.changed().await.unwrap();
        }
    })
    .await
    .expect("refreshed");
    // The new token is good for a day: the refresher goes back to sleep
    // (the mock's expect(1) fails the test on a second refresh).
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!task.is_finished());
    task.abort();
}

#[tokio::test]
async fn token_refresher_stops_when_login_is_required() {
    let server = MockServer::start().await;
    Mock::given(path("/api/auth/tokenRefresh"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"code": 10201, "message": "AUTH_INVALID_REQUEST"})),
        )
        .mount(&server)
        .await;
    let tokens = Tokens {
        access_token: Some("ACCESS-1".into()),
        refresh_token: Some("REFRESH-1".into()),
        token_issue_time_epoch_sec: Some(now_sec() - 10),
        duration_until_refresh_in_sec: Some(5),
        ..Default::default()
    };
    let client = Client::with_tokens(config(&server), tokens).unwrap();
    let res = tokio::time::timeout(std::time::Duration::from_secs(10), client.run_token_refresher())
        .await
        .expect("returns");
    assert!(matches!(res, Err(Error::Reauth(_))), "{res:?}");
}

#[tokio::test]
async fn group_management_wire_shapes() {
    let server = MockServer::start().await;
    let talk = |m: &str| format!("/api/talk/thrift/Talk/TalkService/{m}");
    Mock::given(path(talk("createChat")))
        .respond_with(|req: &Request| {
            let a: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(a[0]["type"], 0);
            assert_eq!(a[0]["name"], "Trip");
            assert_eq!(a[0]["targetUserMids"], json!(["Ua", "Ub"]));
            ok(json!({"chat": {"chatMid": "Cnew", "chatName": "Trip", "type": 0}}))
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path(talk("updateChat")))
        .respond_with(|req: &Request| {
            let a: Value = serde_json::from_slice(&req.body).unwrap();
            let r = &a[0];
            assert!(r["reqSeq"].is_i64());
            match r["updatedAttribute"].as_i64().unwrap() {
                1 => assert_eq!(
                    r["chat"],
                    json!({"chatMid": "Cnew", "chatName": "Trip 2", "type": 0})
                ),
                8 => assert_eq!(
                    r["chat"],
                    json!({"chatMid": "Cnew", "type": 0, "notificationDisabled": true})
                ),
                other => panic!("attribute {other}"),
            }
            ok(json!({}))
        })
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path(talk("updateContactSetting")))
        .respond_with(|req: &Request| {
            let a: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!((&a[1], &a[2], &a[3]), (&json!("Ua"), &json!(1), &json!("false")));
            ok(Value::Null)
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path(talk("updateSettingsAttributes2")))
        .and(body_json_partial_index(1, json!([2])))
        .respond_with(ok(Value::Null))
        .expect(1)
        .mount(&server)
        .await;
    let c = logged_in(&server);
    let chat = c.create_group("Trip", &["Ua".into(), "Ub".into()]).await.unwrap();
    assert_eq!(chat.chat_mid, "Cnew");
    c.rename_chat("Cnew", "Trip 2").await.unwrap();
    c.set_chat_muted("Cnew", true).await.unwrap();
    c.set_chat_muted("Ua", false).await.unwrap();
    c.update_settings(&[2], json!({"notificationNewMessage": false}))
        .await
        .unwrap();
}

#[tokio::test]
async fn create_group_validates_input_and_answer() {
    let server = MockServer::start().await;
    Mock::given(path("/api/talk/thrift/Talk/TalkService/createChat"))
        .respond_with(|req: &Request| {
            let a: Value = serde_json::from_slice(&req.body).unwrap();
            // Trimmed name, deduplicated members without blanks.
            assert_eq!(a[0]["name"], "Trip");
            assert_eq!(a[0]["targetUserMids"], json!(["Ua", "Ub"]));
            ok(json!({"chat": {"chatName": ""}}))
        })
        .expect(1)
        .mount(&server)
        .await;
    let c = logged_in(&server);
    assert!(c.create_group("  ", &["Ua".into()]).await.is_err());
    assert!(c.create_group("Trip", &[" ".into()]).await.is_err());
    let res = c
        .create_group(" Trip ", &["Ua".into(), "".into(), "Ub".into(), "Ua".into()])
        .await;
    assert!(
        res.is_err(),
        "an answer without a chat mid is not a group: {res:?}"
    );
}

/// Matches requests whose JSON array has `value` at `index`.
fn body_json_partial_index(index: usize, value: Value) -> impl wiremock::Match {
    struct M(usize, Value);
    impl wiremock::Match for M {
        fn matches(&self, req: &Request) -> bool {
            serde_json::from_slice::<Value>(&req.body).is_ok_and(|v| v[self.0] == self.1)
        }
    }
    M(index, value)
}

#[tokio::test]
async fn owned_stickers_and_read_ranges() {
    let server = MockServer::start().await;
    Mock::given(path(
        "/api/shop/thrift/ShopService/ShopService/getOwnedProductSummaries",
    ))
    .and(body_json_partial_index(0, json!("stickershop")))
    .respond_with(ok(json!({"offset": 0, "totalSize": 1, "productList": [{
            "id": "13325", "name": "Crayon", "latestVersion": "1",
            "productTypeSummary": {"stickerSummary": {"stickerResourceType": 2,
                "stickerIdRanges": [{"size": 3, "start": "139757086"}]}}}]})))
    .expect(1)
    .mount(&server)
    .await;
    Mock::given(path("/api/talk/thrift/Talk/TalkService/getMessageReadRange"))
        .and(body_json(json!([["Cg"], 3])))
        .respond_with(ok(json!([{"chatId": "Cg", "ranges": {"Ua": [
            {"startMessageId": "1", "endMessageId": "5", "startTime": "1", "endTime": "50"},
            {"startMessageId": "7", "endMessageId": "12", "startTime": "70", "endTime": "120"}]}}])))
        .expect(1)
        .mount(&server)
        .await;
    let c = logged_in(&server);
    let p = c.owned_sticker_packages().await.unwrap();
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].sticker_ids, ["139757086", "139757087", "139757088"]);
    assert!(p[0].animated);
    let r = c.message_read_ranges(&["Cg".into()]).await.unwrap();
    assert_eq!(
        (r[0].reader.as_str(), r[0].end_message_id.as_str(), r[0].end_time),
        ("Ua", "12", 120)
    );
}

fn sse(body: &'static str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

async fn stream_opens(server: &MockServer) -> usize {
    let requests = server.received_requests().await.unwrap();
    requests
        .iter()
        .filter(|r| r.url.path() == "/api/operation/receive")
        .count()
}

#[tokio::test]
async fn unproductive_stream_connections_back_off() {
    let server = MockServer::start().await;
    // Accepted, then closed at once without a single event.
    Mock::given(method("GET"))
        .and(path("/api/operation/receive"))
        .respond_with(sse(""))
        .mount(&server)
        .await;
    let client = logged_in(&server);
    let mut ops = client.operations(Some(100));
    let _ = tokio::time::timeout(std::time::Duration::from_millis(1500), ops.next_event()).await;
    // At 0 s, then after 1 s of backoff (the next one would wait 2 s more).
    let opened = stream_opens(&server).await;
    assert!((1..=3).contains(&opened), "opened {opened} times in 1.5 s");
}

#[tokio::test]
async fn stream_survives_a_failed_token_refresh() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/operation/receive"))
        .respond_with(sse("event: talkException\ndata: {\"code\":119}\n\n"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/operation/receive"))
        .respond_with(sse(
            "data: [{\"revision\":\"101\",\"type\":55,\"param1\":\"c1\"}]\n\n",
        ))
        .mount(&server)
        .await;
    // The refresh hits a server error: retrying later may help, so the stream goes on.
    Mock::given(path("/api/auth/tokenRefresh"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let client = logged_in(&server);
    let mut ops = client.operations(Some(100));
    let op = tokio::time::timeout(std::time::Duration::from_secs(10), ops.next_operation())
        .await
        .expect("no hang")
        .expect("the stream must not stop on a transient refresh failure");
    assert_eq!(op.revision, 101);
}

#[tokio::test]
async fn an_operation_that_stops_the_stream_is_delivered_again_on_resume() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/operation/receive"))
        .respond_with(sse(concat!(
            "data: [{\"revision\":\"101\",\"type\":55,\"param1\":\"c1\"},",
            "{\"revision\":\"102\",\"type\":26,\"message\":{\"id\":\"5\",\"from\":\"uA\",\"to\":\"uB\",",
            "\"contentType\":0,\"chunks\":[\"AAAA\",\"AAAA\",\"AAAA\"]}}]\n\n"
        )))
        .mount(&server)
        .await;
    // Logged in without our mid: decrypting 102 fails with a session error.
    let tokens = Tokens {
        access_token: Some("ACCESS-1".into()),
        ..Default::default()
    };
    let client = Client::with_tokens(config(&server), tokens).unwrap();
    let mut ops = client.operations(Some(100));
    ops.decrypt = true;
    let err = ops.next_event().await.unwrap_err();
    assert!(matches!(err, Error::LoginRequired), "{err:?}");
    assert_eq!(ops.local_rev, Some(101), "102 was not delivered");
    // 101 was handled before the error and is still delivered.
    assert!(matches!(ops.next_event().await, Ok(rune::Event::Operation(op)) if op.revision == 101));
}

#[tokio::test]
async fn refresh_retries_end_even_when_the_policy_never_backs_off() {
    let server = MockServer::start().await;
    Mock::given(path("/api/auth/tokenRefresh"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"code": 10202, "message": "AUTH_RETRY_REQUIRED"})),
        )
        .mount(&server)
        .await;
    let tokens = Tokens {
        access_token: Some("ACCESS-1".into()),
        refresh_token: Some("REFRESH-1".into()),
        refresh_api_retry_policy: serde_json::from_value(json!({
            "initialDelayInMillis": 0, "maxDelayInMillis": 1000, "multiplier": 1, "jitterRate": 0
        }))
        .ok(),
        ..Default::default()
    };
    let client = Client::with_tokens(config(&server), tokens).unwrap();
    let res = tokio::time::timeout(std::time::Duration::from_secs(10), client.refresh_token())
        .await
        .expect("gives up instead of retrying forever");
    assert!(matches!(res, Err(Error::Reauth(_))), "{res:?}");
}

#[tokio::test]
async fn token_refresher_does_not_spin_on_an_already_due_token() {
    let server = MockServer::start().await;
    // LINE's clock is behind ours: every new token already looks due.
    Mock::given(path("/api/auth/tokenRefresh"))
        .respond_with(ok(json!({"tokenV3IssueResult": {
            "accessToken": "ACCESS-2", "refreshToken": "REFRESH-2",
            "tokenIssueTimeEpochSec": (now_sec() - 7200).to_string(), "durationUntilRefreshInSec": "3600"}})))
        .mount(&server)
        .await;
    let tokens = Tokens {
        access_token: Some("ACCESS-1".into()),
        refresh_token: Some("REFRESH-1".into()),
        token_issue_time_epoch_sec: Some(now_sec() - 7200),
        duration_until_refresh_in_sec: Some(3600),
        ..Default::default()
    };
    let client = Client::with_tokens(config(&server), tokens).unwrap();
    let task = client.spawn_token_refresher();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    task.abort();
    let n = server.received_requests().await.unwrap().len();
    assert_eq!(n, 1, "refreshed {n} times in a second");
}

#[tokio::test]
async fn a_failed_qr_login_frees_its_key() {
    let server = MockServer::start().await;
    let svc = "/api/talk/thrift/LoginQrCode/SecondaryQrCodeLoginService";
    let notice = "/api/talk/thrift/LoginQrCode/SecondaryQrCodeLoginPermitNoticeService";
    Mock::given(path(format!("{svc}/createSession")))
        .respond_with(ok(json!({"authSessionId": "S1"})))
        .mount(&server)
        .await;
    Mock::given(path(format!("{svc}/createQrCode")))
        .respond_with(ok(json!({"callbackUrl": "https://line.me/R/au/lgn/sq/S1",
                                 "longPollingIntervalSec": 1, "longPollingMaxCount": 1})))
        .mount(&server)
        .await;
    // The scan is refused (not a 410 "poll again").
    Mock::given(path(format!("{notice}/checkQrCodeVerified")))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"code": 10051, "data": {"code": 20}})))
        .mount(&server)
        .await;
    let client = Client::new(config(&server)).unwrap();
    let opts = QrLoginOptions {
        wait: std::time::Duration::from_secs(1),
        ..Default::default()
    };
    let err = client.qr_login(opts, |_| {}).await.unwrap_err();
    assert!(err.api().is_some(), "{err:?}");
    assert!(client.with_ltsm(|l| format!("{l:?}")).contains("keys: 0"));
}
