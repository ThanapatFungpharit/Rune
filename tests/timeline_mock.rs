//! Profile covers and album photos against a mock gateway and OBS.

use base64::Engine as _;
use rune::{Client, Config, Message, Tokens};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const ME: &str = "u00000000000000000000000000000001";

fn ok(data: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"code": 0, "message": "OK", "data": data}))
}

fn client(server: &MockServer) -> Client {
    let tokens = Tokens {
        access_token: Some("ACCESS-1".into()),
        refresh_token: Some("REFRESH-1".into()),
        mid: Some(ME.into()),
        ..Default::default()
    };
    let config = Config {
        gateway: server.uri(),
        obs: server.uri(),
        max_retries: 0,
        ..Config::default()
    };
    Client::with_tokens(config, tokens).unwrap()
}

/// `issueChannelToken` for the timeline channel, and our profile (for `regionCode`).
async fn mount_basics(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/talk/thrift/Talk/ChannelService/issueChannelToken"))
        .and(body_json(json!(["1341209850"])))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"message": "OK", "data": {
                "channelAccessToken": "CHAN-1", "expiration": "9999999999999", "token": "x"
            }})),
        )
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/talk/thrift/Talk/TalkService/getProfile"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"message": "OK", "data": {"mid": ME, "regionCode": "TH"}})),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn cover_is_fetched_and_downloaded_with_the_channel_token() {
    let server = MockServer::start().await;
    mount_basics(&server).await;
    let friend = "u00000000000000000000000000000002";
    Mock::given(method("GET"))
        .and(path("/api/timeline/getCover"))
        .and(query_param("targetMid", friend))
        .and(query_param("myMid", ME))
        .and(query_param("myRegion", "TH"))
        .and(header("x-line-channeltoken", "CHAN-1"))
        .and(header("x-line-access", "ACCESS-1"))
        .respond_with(|req: &Request| {
            // Signed over the path and query, like any gateway GET.
            let pq = format!("{}?{}", req.url.path(), req.url.query().unwrap());
            let want = rune::ltsm::Ltsm::new()
                .unwrap()
                .hmac("ACCESS-1", &pq, "")
                .unwrap();
            assert_eq!(req.headers.get("x-hmac").unwrap().to_str().unwrap(), want);
            ok(json!({
                "coverObsInfo": {"objectId": "abc", "obsNamespace": "c", "serviceName": "myhome"},
                "isDefaultCover": false, "meta": {}, "profileId": "p"
            }))
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/r/myhome/c/abc"))
        .and(header("x-line-channeltoken", "CHAN-1"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"JPEG".to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    let c = client(&server);
    let cover = c.profile_cover(friend).await.unwrap();
    assert_eq!(cover.custom_image().unwrap().path(), "/r/myhome/c/abc");
    assert_eq!(c.download_cover(&cover).await.unwrap().unwrap(), b"JPEG");
    // A myhome request never carries the encrypted OBS token.
    for r in server.received_requests().await.unwrap() {
        if r.url.path().starts_with("/r/") {
            assert!(r.headers.get("x-line-access").is_none());
        }
    }
}

#[tokio::test]
async fn groups_have_no_cover() {
    let server = MockServer::start().await;
    let c = client(&server);
    assert!(
        c.profile_cover("c00000000000000000000000000000001")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn set_profile_cover_uploads_then_updates() {
    let server = MockServer::start().await;
    mount_basics(&server).await;
    Mock::given(method("POST"))
        .and(path_regex_reqid())
        .and(header("x-line-channeltoken", "CHAN-1"))
        .respond_with(|req: &Request| {
            let p = req.headers.get("x-obs-params").unwrap().to_str().unwrap();
            let p: Value =
                serde_json::from_slice(&base64::engine::general_purpose::STANDARD.decode(p).unwrap())
                    .unwrap();
            assert_eq!(p, json!({"ver": "2.0", "name": "me.png", "type": "image"}));
            assert_eq!(req.body, b"not really a png");
            ResponseTemplate::new(201)
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/timeline/updateCover"))
        .and(header("x-line-channeltoken", "CHAN-1"))
        .respond_with(|req: &Request| {
            let b: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(b["myMid"], ME);
            assert_eq!(b["myRegion"], "TH");
            assert_eq!(b["storyShare"], false);
            // The object id is the upload's reqid, as the extension sends it.
            assert!(b["coverObjectId"].as_str().unwrap().starts_with("reqid-"));
            ok(json!({"coverObsInfo": {"objectId": "new", "obsNamespace": "c", "serviceName": "myhome"}}))
        })
        .expect(1)
        .mount(&server)
        .await;

    let c = client(&server);
    let o = c.set_profile_cover(b"not really a png", "me.png").await.unwrap();
    assert_eq!(o.path(), "/r/myhome/c/new");
}

#[tokio::test]
async fn album_post_media_needs_the_group_mid() {
    let server = MockServer::start().await;
    mount_basics(&server).await;
    Mock::given(method("GET"))
        .and(path("/r/album/a/oid1"))
        .and(header("x-line-channeltoken", "CHAN-1"))
        .and(header("x-line-mid", "cGROUP"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PHOTO".to_vec()))
        .expect(2)
        .mount(&server)
        .await;

    let mut m = Message {
        id: "1".into(),
        content_type: 16,
        to: "cGROUP".into(),
        ..Default::default()
    };
    for (k, v) in [
        ("serviceType", "AB"),
        ("locKey", "BT"),
        ("chatId", "cGROUP"),
        (
            "postEndUrl",
            "line://group/home/albums/album?albumId=7&homeId=H&albumIdV2=7",
        ),
        (
            "previewMedias",
            r#"[{"svc":"album","sid":"a","mediaOid":"oid1","mediaType":"I"}]"#,
        ),
    ] {
        m.content_metadata.insert(k.into(), v.into());
    }
    let c = client(&server);
    // `download_media` understands album posts (previews are the photo itself).
    assert_eq!(c.download_media(&m, false).await.unwrap(), b"PHOTO");
    assert_eq!(c.download_media(&m, true).await.unwrap(), b"PHOTO");
}

fn path_regex_reqid() -> wiremock::matchers::PathRegexMatcher {
    wiremock::matchers::path_regex(r"^/r/myhome/c/reqid-[0-9a-f-]{36}$")
}

#[tokio::test]
async fn timeline_calls_refresh_the_access_token_when_asked() {
    let server = MockServer::start().await;
    mount_basics(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/timeline/homeId"))
        .and(header("x-line-access", "ACCESS-1"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": 10051, "message": "RESPONSE_ERROR", "data": {"name": "TalkException", "code": 119}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/auth/tokenRefresh"))
        .respond_with(ok(
            json!({"tokenV3IssueResult": {"accessToken": "ACCESS-2", "refreshToken": "REFRESH-2"}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/timeline/homeId"))
        .and(header("x-line-access", "ACCESS-2"))
        .respond_with(ok(json!("HOME-1")))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(client(&server).timeline_home_id(ME).await.unwrap(), "HOME-1");
}
