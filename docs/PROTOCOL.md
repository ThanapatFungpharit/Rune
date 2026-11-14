# Protocol notes

What `rune` knows about the LINE Chrome-extension protocol (`CHROMEOS` 3.7.2),
where that knowledge comes from, and how sure we are. The main source is the
reference project [OkLine](https://github.com/NiceDayZc/OkLine) (Python,
reverse-engineered from the extension bundle, commit `2e34379`, Sept 2026).

**Legend**
- **verified**: checked by `rune` against an independent oracle (the
  reference implementation running LINE's own JS glue) or by tests.
- **live**: exercised by `rune` against the real service (see "Live run").
- **reference**: taken from OkLine (its docs say *live-tested* or *from the
  bundle*); `rune` reproduces it but has not exercised it live yet.
- **inferred**: guessed or unconfirmed even in the reference.

## Scope and limitations

This document describes observed behavior of the LINE ChromeOS extension
protocol (`CHROMEOS` 3.7.2). It is reverse-engineered documentation, not
an official protocol specification.

- All live observations are specific to the account, region and protocol
  version used during testing.
- Undocumented endpoints, request formats and response structures may
  change without notice.
- Successful requests do not establish that an endpoint is officially
  supported or intended for third-party use.
- Cryptographic interoperability tests demonstrate compatibility with
  the tested reference implementation, not an independent security
  certification.
- Reference-derived behavior must not be treated as independently
  verified by `rune` until explicitly marked as such.

`rune` is an independent implementation and is not affiliated with or
endorsed by LY Corporation.

## Transport

| Item | Detail | Status |
|---|---|---|
| Gateway | `https://line-chrome-gw.line-apps.com` | reference |
| RPC | `POST /api/<ns>/thrift/<Namespace>/<Service>/<method>`; body is a JSON **array of positional args**; structs are objects with camelCase field names | reference |
| Envelope | success `{"message":"OK","data":…}` (exactly `"OK"`); anything else, including a 2xx without the envelope, is an error | reference |
| Errors | outer gateway code (`10051` RESPONSE_ERROR, `10052` HTTP_ERROR with `data.statusCode`, `10006` MUST_UPGRADE, `99999` UNKNOWN) wrapping an inner `TalkException` `{code, reason}` | reference |
| Headers | `X-Line-Access`, `X-Line-Chrome-Version: 3.7.2`, `X-LAL: en_US`, `Accept-Language`, `content-type: application/json` (only with a body), `X-Hmac` | reference |
| `X-Line-Application` | `CHROMEOS\t3.7.2\tChrome_OS\t`, **only** on private OBS requests, never on the gateway | reference |
| int64 | often serialized as JSON **strings** (`revision`, `createdTime`, token times); `rune` accepts both | reference |
| Retries | 5xx / network errors, outer `99999`, inner `115` SHOULD_RETRY | reference |
| Token renewal (refresh endpoint live) | inner `119` MUST_REFRESH_V3_TOKEN on `/api/talk/thrift/Talk*` (minus `Talk/ChannelService`, `Talk/E2EEKeyBackupService`) → `POST /api/auth/tokenRefresh {"refreshToken"}` (must also carry `X-Line-Access`) → replay once; `10201` = re-login, `10202` = retry with `refreshApiRetryPolicy` backoff | reference |
| Kick-out | inner `1`/`7`/`8` in the same path scope | reference |
| `reqSeq` | per-request sequence number; reference restarts at 1 per process, `rune` seeds from the clock | inferred |

## Live run, 2026-11-13

One personal account, all **live**:

- QR login with PIN → tokens, certificate and 3 keychain keys captured.
- `getProfile`, `getAllChatMids`/`getChats` (135 groups), `getAllContactIds`
  (169; `getContactsV2` rejects 169 mids in one call, batches of 100 work),
  `getRecentMessagesV2`, `getMessageBoxes`, `getServerTime`,
  `getLastOpRevision`, `getLastE2EEGroupSharedKey`.
- SSE connect; the server sends `connInfoRevision` on connect and `ping` every 20 s.
- `/api/auth/tokenRefresh`, then calls with the new token.
- Letter Sealing: 37/37 sealed history messages decrypted (1:1 + group);
  one sealed 1:1 and one sealed group text sent and seen on the phone.

- Operation stream replayed from an older `localRev`: delivered every later
  operation; sealed group messages from another member's official app and
  from the account's phone were decrypted on the fly. Operations caused by
  this client's own session are not echoed back.
- E2EE key state: `getE2EEPublicKeysEx` lists the account's current key(s);
  `getE2EEPublicKey(mid, 1, keyId)` answers 85 "invalid keyId" for retired
  key ids **and for key id 0** (so the reference's "id 0 = latest" fallback
  does not work).

Not yet observed live: the 119 refresh path, E2EE retry codes 83/84/99,
creating a group key, an account key rotation, non-text sealed types.

Observed: mids have the newer `U`/`C` + 43-char base64url form (authenticated,
see Letter Sealing); `getServerTime`/`getLastOpRevision`/`createdTime` arrive
as JSON strings; `refreshApiRetryPolicy` was `{200 ms, 104857600 ms, ×2, 0.3}`.

## `X-Hmac` signing (verified, live)

From `ltsmSandbox.js` (`get_hmac` handler):

```text
tokenKey = SecureKey.loadToken("wODdrvWqmd…")       # per-extension-origin token
key      = tokenKey.deriveKey(SHA256("3.7.2"), SHA256(accessToken))
X-Hmac   = base64(new Hmac(key).digest(utf8(path + body)))
```

`path` includes the query string (GETs sign `path?query` with an empty body),
`accessToken` is `""` before login. `deriveKey`/`Hmac` are custom C++ inside
`ltsm.wasm`, so `rune` runs that module rather than re-implementing it:

- `src/ltsm/runtime.rs` implements the module's 40 Emscripten imports for
  wasmtime: syscall stubs (`/dev/urandom` only), `EM_ASM` random snippets,
  `window.location.origin` (the module checks it), Embind type/class/method
  registration, and a minimal `emval` table (read `Uint8Array`s, build
  `Uint8Array`s from memory views, number conversion).
- Classes found via Embind: `SecureKey`, `Hmac`, `AesKey`, `Curve25519Key`,
  `E2EEKey`, `E2EEChannel`, `E2EEKeychain` (signatures are printed by
  `Ltsm::describe()`).
- **Verification:** `tests/fixtures/hmac_vectors.json` holds 8 vectors produced
  by OkLine's Node bridge (LINE's real JS glue + the same wasm): empty/real
  tokens, GET query strings, Thai/emoji/CJK bodies, long bodies. `rune`
  matches all of them byte-for-byte. Regenerate with
  `node scripts/gen_hmac_vectors.js <OkLine>/okline/ltsm`.

The import-letter mapping (`a.a` … `a.N`) and the `EM_ASM` addresses are
specific to this wasm build (sha256 `58bb4e18…`). A new `ltsm.wasm` from a newer
extension needs them re-derived from its `ltsmSandbox.js`.

## QR login (live)

```text
LoginQrCode/SecondaryQrCodeLoginService/createSession  [{}]          -> authSessionId
                                   …/createQrCode      [{authSessionId}] -> callbackUrl, longPollingIntervalSec, longPollingMaxCount
QR = callbackUrl + ?secret=<b64 Curve25519 pubkey from ltsm>&e2eeVersion=1
LoginQrCode/SecondaryQrCodeLoginPermitNoticeService/checkQrCodeVerified
      headers X-Line-Session-ID, X-LST=<interval ms>; retry on status 410
…/verifyCertificate [{authSessionId, certificate}]   ok -> skip PIN
…/createPinCode -> pinCode;  …/checkPinCodeVerified (X-LST 110000, retry 408/410)
…/qrCodeLoginV2 [{systemName:"CHROMEOS", modelName:"CHROME", autoLoginIsRequired:false, authSessionId}]
      -> tokenV3IssueResult, mid, certificate, metaData{publicKey, encryptedKeyChain}
```

Pre-login requests are signed with the empty token and carry no
`X-Line-Access`. The keychain is unwrapped with
`Curve25519Key.createChannel(metaData.publicKey).unwrapE2EEKeyChain(...)`;
`rune` exports each key (`E2EEKey.exportKey`) into the session file because the
keychain is **only** delivered at login.

E-mail/password login (`loginV2` with RSA-encrypted credentials) is not
implemented.

## Operation stream (connect + pings live; delivery mock-tested)

`GET /api/operation/receive?version=3.7.2&language=en_US&lastPartialFullSyncs={}&localRev=<rev>`
returns `text/event-stream`. Unnamed events hold `{"operations":[…]}` (or a
list, or one operation). Named events: `ping`, `connInfoRevision`,
`reconnect`, `talkException`, `fullSync`/`partialFullSync` (carry
`nextRevision`). `localRev` is seeded from `getLastOpRevision`, advanced by
every operation and re-sent on reconnect; already-seen revisions are dropped.
The extension authenticates the `EventSource` with a cookie; like the
reference, `rune` sends `X-Line-Access` + `X-Hmac` instead (a deviation).

Reconnect backoff `min(2^n s, 60 s)` for unproductive connections; a
productive connection reconnects immediately. The server pings every 20 s
(live), so a 30 s silence watchdog (the extension's 20 s interval + 10 s
spare) reopens dead connections.

## Group invitations (listing + reject live; accept from reference)

- `getAllChatMids([{withMemberChats, withInvitedChats: true}, 4])` →
  `invitedChatMids` lists pending invitations (live: a real invitation showed
  up there and `getChats` returned its name and members).
- `acceptChatInvitation([{reqSeq, chatMid}])` / `rejectChatInvitation([{reqSeq,
  chatMid}])`: shapes from the reference (`okline/services/chats.py`); reject
  confirmed live (2026-11-14, a spam invitation declined from rune), accept not
  yet exercised live.
- Operation `124` `NOTIFIED_INVITE_INTO_CHAT`: `param1` is the chat mid. The
  reference reads `param2` as the inviter; whether the op also reaches members
  when *others* are invited is unconfirmed, so the TUI re-reads the chat with
  `getChats` and only lists it if our mid is in `groupExtra.inviteeMids`.

## Replies and forwards (receive side live, 2026-11-14)

Replies are **not** a content type (content type 16 is `POSTNOTIFICATION`).
A reply is an ordinary message of any type (text, sticker, sealed text, …)
with top-level `messageRelationType: 3` (REPLY), `relatedMessageId: "<id>"`
and `relatedMessageServiceCode: 1` (TALK). Forwards carry the same fields with
`messageRelationType: 0` (FORWARD). Messages from current official apps also
repeat the relation in `contentMetadata`: `message_relation_server_message_id`,
`message_relation_service_code: "talk"`, `message_relation_type_code:
"reply"|"forward"`; older/other senders omit those. On sealed messages all of
these stay in plain sight; only `text` is encrypted. (Live: 39 chats, 84
related messages: 40 replies, 44 forwards.)

The extension never builds replies: the only place it touches these fields
is where it deletes them when sharing/forwarding a message (`Ob` in
`ltsmSandbox.js`). `rune` sends the reference's three top-level fields plus the
metadata copy, like the official apps; the server's acceptance of a reply sent
through the Chrome gateway is not verified live yet.

`getMessagesByIds` (registered, never called by the extension) rejected every
argument shape tried with `10102 THRIFT_INVALID_ARGUMENT` (`[[id]]`,
`[[id], 3]`, `[[{messageId, messageBoxId}], 3]`, `[[{messageId, chatMid}], 3]`)
or `99999` (`[{messageIds}, 3]`), so a quoted message older than the loaded
history cannot be fetched by id; the TUI shows "an earlier message".

## OpenChat (Square): not available (live, 2026-11-14)

The extension registers no `SquareService` method, and the gateway only serves
the methods the extension registers: even real `TalkService` methods it does
not use (`getContact`, `getChatRoomAnnouncements`) return HTTP 404 with gateway
code `10001` `REQUEST_NOT_FOUND`. So do `getJoinedSquares` / `getInvitedSquares`
under every path tried (`talk/thrift/Square/SquareService`,
`talk/thrift/Talk/SquareService`, `square/thrift/Square/SquareService`,
`square/thrift/SquareService/SquareService`, `talk/thrift/SQ1/SquareService`,
`sq/…`, `openchat/…`). The native endpoint (`/SQ1`, Thrift compact) on the
LEGY hosts (`legy-jp.line-apps.com`, `gw.line.naver.jp`, `ga2/gd2.line.naver.jp`)
did not answer from this machine (timeouts; `legacy-gw.line-apps.com` no longer
resolves), and a CHROMEOS token is unlikely to be accepted there anyway.
OpenChat would need a separate native-protocol client.

## Letter Sealing / E2EE

Implemented in `src/e2ee.rs` for text and location messages, 1:1 and group,
following the reference (which reports these flows live-verified).

| Piece | Status |
|---|---|
| Curve25519 keygen, public key, ECDH channels | verified (round-trip, and interop with the reference bridge in both directions) |
| V1 encrypt/decrypt | verified (`v1_interop_with_reference_bridge`); no V1 message seen live |
| Keychain unwrap, key id, export/load | live: 3 keys unwrapped at login, exported, re-loaded in new processes |
| V2 encrypt/decrypt | verified with real keys, incl. AAD tamper rejection |
| Decrypting history | **live**: 37/37 sealed messages (3 one-to-one, 3 group chats; incoming and own) |
| Decrypting the live stream | **live**: messages sealed by another member's official app and by the phone, decrypted as they arrived (replayed stream) |
| Sending sealed, 82 fallback | **live**: one 1:1 and one group text, shown correctly on the phone |
| Typed failures, kept input | mock-tested with real keys (`tests/e2ee_lifecycle.rs`): missing own key, retired peer key, tampered/corrupted chunks, unsupported version, no keys |
| Key rotation: peer (84 → renegotiate + resend to new key), own key outdated (83), cache drop on key/membership operations | mock-tested with real keys; not observed live |
| Group key registration (`registerE2EEGroupKey`) | mock-tested (second member unwraps and decrypts); not run live |
| Restart | live (every CLI run is a new process) and mock-tested (`state_survives_restart`) |
| Sealed media (`ENC_KM`, encrypted OBS blobs) | not implemented |

### Key lifecycle as implemented

- **Our keys** arrive only with the keychain at QR login (there is no other
  source in the reference). They are stored as exported blobs in the session
  and reloaded on start; all of them are kept, so messages sealed to older
  keys still decrypt. Public halves of our own keys are served locally,
  because the server stops serving retired ids.
- **Detecting an outdated keychain**: `Client::e2ee_status()` compares local
  ids with `getE2EEPublicKeysEx`; a send rejected with 83
  (`E2EE_UPDATE_SENDER_KEY`) becomes `E2eeErrorKind::OwnKeyOutdated`; a
  message sealed to an id we lack is `MissingOwnKey`. Recovery in all three
  cases: QR login again (the phone sends the new keychain).
- **Peer keys** are fetched by the key id carried in each message (immutable,
  cached). For sending, the negotiated key is cached per chat and dropped on
  84/86/87/88/90 (resend, up to 3 times), on 99 (new group key) and when a
  key-change or membership operation mentions the chat. Which `param` of
  those operations carries the mid is not documented in the reference, so
  every param is matched against cached chats (local invalidation only).
- **Failures never drop messages**: `decrypt_message` leaves its input
  untouched; the stream attaches `Operation::e2ee_error` and still yields the
  sealed original; unparsable operations are yielded as `unparsed` control
  events; decrypted messages are cached by id (1024) so duplicates are cheap.
- Not implemented (not in the reference): `E2EEKeyBackupService`,
  resend requests (`NOTIFIED_E2EE_MESSAGE_RESEND_REQ/RESP`, ops 75/76).

**mids are authenticated input.** `encryptV2`/`decryptV2` parse the `to`/`from`
mids. New-style mids (`U`/`C` + 43 base64url chars) are MAC-protected tokens,
and any made-up mid (old hex style included) fails with "Ciphertext MAC is
invalid: data authentication failure". That is why early experiments seemed to
show synthetic keys being refused: with a real mid, synthetic keys work for V2
too. Only `exportKey` really is limited to keychain keys ("illegal operation").
Offline tests therefore need a real mid; the E2EE tests read one from a session
file instead of committing it.

Wire format: `chunks = [ct[0:16], ct[28:], ct[16:28], BE32(senderKeyId),
BE32(receiverKeyId)]`, decrypt input `c0 + c2 + c1` (V1: in order, `[8, body,
16]`); plaintext is compact JSON `{text, location, REPLACE}`; the sealed message
drops `text`/`location`/`from` and sets `contentMetadata.e2eeVersion`. A plain
send to a sealed chat fails with `TalkException` 82; `rune` then seals and
resends, and seals later sends to that chat up front. V2 `sequenceNumber` is a
per-client counter seeded from the clock (the reference counts from 0 per
process; the receiver does not check it).

## Media (`src/media.rs`)

Source: the extension's own code, which `ltsmSandbox.js` bundles (minified
names `mo`, `yb`/`vb`, `nE`/`oE`/`sE`/`aE`, `Rb`, `Hb`/`Ry`, `gv`/`vv`/`mv`/`bv`,
`SE`). OkLine only implements the plain upload flow and the blob crypto.

| Piece | Detail | Status |
|---|---|---|
| OBS auth | `X-Line-Access: <encrypted token>` (`acquireEncryptedAccessToken(2)` = OBS_GENERAL; live blob = 2 rows, token is row 1 col 0) + `X-Line-Application`; no `X-Hmac`; renew token on 401/403 | live |
| Which flow | `determineMediaMessageFlow({chatMid}).flowMap[contentType]`, 2 = sealed; live both test chats answered V2 for image/video/audio/file, TTL 6 h | live |
| Download paths | sealed `/r/talk/{SID}/{OID}` (+`__ud-preview`), plain `/r/talk/m/{id}` (+`/preview`, `/original`), `?p=OBS_POP`, public `DOWNLOAD_URL`/`PREVIEW_URL` | **live**: sealed image + preview, sealed video (chunked) + preview, sealed PDF, plain image + preview, plain PDF, bot image |
| `X-Talk-Meta` (`SE`) | Thrift-binary {4: id, 27: empty list} → base64 → `{"message":…}` → base64 | matches the reference; accepted live |
| Blob crypto | HKDF-SHA256(ENC_KM, salt ∅, info `FileEncryption`) → AES-256-CTR (nonce‖0⁴, 32-bit counter) + HMAC-SHA256; video: HMAC over the SHA-256 list of 128 KiB ciphertext chunks | byte-identical to the extension's algorithm on WebCrypto (`tests/fixtures/blob_vectors.json`); decrypted live media from the official app |
| Sealed upload (V2) | `ENC_KM` = 32 random bytes; POST blob to `/r/talk/{emi,emv,ema,emf}/reqid-{uuid}` with `X-Obs-Params` `{"ver":"2.0","name":"<ms>","type":"file"}`; real id from the `x-obs-oid` header; then `{oid}__ud-preview`, `{oid}__ud-hash` (video); then the sealed message with `SID`, `OID`, `FILE_SIZE`, `DURATION`, `MEDIA_THUMB_INFO`; `ENC_KM`/`FILE_NAME` go inside the ciphertext | **live** for an image (1:1): stored with the same metadata shape as official-app images (no `ENC_KM`/`FILE_NAME` in clear), downloaded and decrypted back (640×400 + 384×240 preview). Video/audio/file uploads mock-tested only |
| Plain upload (V1) | send the message, then POST to `/r/talk/m/{id}` with type `image/video/audio/file` (`cat:"original"` for original images) | mock-tested |
| Images | preview = fit 384×384 JPEG q0.3 (`MEDIA_THUMB_INFO` = its size); main = ≤1280² pixels JPEG q0.6, or the original bytes with `MEDIA_CONTENT_INFO` (`category:"original"`); >20 MiB → file | rules unit-tested; resize quality differs from a browser canvas |
| Profile / group pictures | `https://profile.line-scdn.net{picturePath}` (+`/preview`, `/small`), public | live |
| Stickers | `https://stickershop.line-scdn.net/stickershop/v1/sticker/{STKID}/android/sticker.png` (`sticker_key.png`, `iPhone/sticker@2x.png`), package icon `…/product/{id}/LINEStorePC/main.png` | live |
| File expiry | `FILE_EXPIRE_TIMESTAMP` passed → server 404; reported as `Error::MediaUnavailable` | live |

Not done: video/audio transcoding (the extension converts to MP4/M4A in the
browser; callers must pass MP4/M4A) and video thumbnail extraction (pass a
preview JPEG yourself); EXIF/GPS stripping for "original" images (the
extension zeroes GPS tags; re-encoded images carry no EXIF); animated/popup
sticker variants (URLs not confirmed); grouped image messages (`GID`/`GSEQ`);
re-forwarding via `/api/obs/copyForMessage`; resumable uploads (`range`).

## Timeline: profile covers (live, 2026-11-14)

Source: the extension's UI bundle (`static/js/main.js` of the 3.7.2 CRX;
OkLine's `ltsmSandbox.js` holds only the plumbing). Code: `src/timeline.rs`.

| Fact | Provenance |
|---|---|
| Channel token: `ChannelService/issueChannelToken(["1341209850"])` → `{channelAccessToken, expiration (ms), …}`; renewed when expired, and once on a 401 | bundle (`renewChannelToken`); live |
| Gateway requests under `/api/timeline/` add `X-Line-ChannelToken` to the usual signed headers (`X-Hmac` over path+query and body); responses use the `{message:"OK", data}` envelope (plus `code: 0`) | bundle (`zU` header mapper); live |
| Only `getCover`, `updateCover` and `homeId` are served; other timeline/album paths (`hm/api/v1/home/cover.json`, `ext/album/api/v3/albums`, …) answer `REQUEST_NOT_FOUND` | bundle has just these three; live |
| `GET getCover?targetMid=&myMid=&myRegion=` (all required; `myRegion` = our `regionCode`) → `{coverObsInfo: {serviceName:"myhome", obsNamespace:"c", objectId}, isDefaultCover, videoCoverObsInfo?: {…, obsNamespace:"vc"}, profileId, meta}` | live (own and friends'); a group mid gives HTTP 500 `UNKNOWN_ERROR` |
| Default cover: `isDefaultCover: true`, `objectId` `c0000000000000000000000000000001` | live |
| Picture: OBS `/r/{serviceName}/{obsNamespace}/{objectId}` (`WO`); `/r/myhome/…` objects authenticate with `X-Line-ChannelToken` only (no `X-Line-Access`/`X-Line-Application`) | bundle (`mo`); live (JPEG, ~100 KB); size suffixes like `/m` give 400 |
| `GET homeId?eMid=<mid>` → the user's timeline home id (a string) | bundle; live |
| Set cover (`KJ`): POST the image to `/r/myhome/c/reqid-{uuid}` with `X-Obs-Params: base64({"ver":"2.0","name":<file name>,"type":"image"})`, then `POST updateCover {myMid, myRegion, coverObjectId: "reqid-…", storyShare: false}` → `{coverObsInfo}` | bundle; **mock-tested only** |

## Group albums (posts live, 2026-11-14)

Code: `src/album.rs`. The extension has no album browser and no album API;
it only renders album chat events. Albums therefore cannot be listed,
created or edited through this gateway.

| Fact | Provenance |
|---|---|
| Album posts: `contentType` 16 (`POSTNOTIFICATION`), metadata `serviceType: "AB"`, `locKey` `BA` (album created) / `BT` (photos added), `albumName`, `cafeId` (= album id), `chatId` (hex group mid), `postEndUrl: line://group/home/albums/album?albumId=…&homeId=…&albumIdV2=…`, `previewMedias: [{svc:"album", sid:"a", mediaOid, mediaType:"I"\|"V"}]`, plus `mediaOid`/`mediaType`/`width`/`height` of the first photo, `mediaCount` (seen `"0"`) | live history (9 posts) |
| The `homeId` in `postEndUrl` equals the group mid the gateway uses for the chat (`C…` form); `chatId` is the hex `c…` form | live |
| Photos: OBS `/r/album/a/{mediaOid}` with `X-Line-ChannelToken` **and** `X-Line-Mid: <chatId>`. The encrypted access token (`X-Line-Access`), or the channel token without `X-Line-Mid` (or with our own mid / the `homeId`), get 403; `/w580`, `/preview` etc. get 400, `/original` 404 | live (all 9 photos, JPEG 58–246 KB); the extension never fetches these |
| Album events: `contentType` 18 with `LOC_KEY` `BB` (renamed; `LOC_ARGS` old␞new), `BD` (deleted; name), `BO` (items removed; name). `LOC_ARGS` splits on U+001E, then U+001F (`VR`). English texts `chat.msg.album.rename/delete/removephoto` | bundle; not seen live yet |

## Chat list paging (live, 2026-11-14)

`getMessageBoxes([{minChatId, activeOnly, unreadOnly, messageBoxCountLimit,
withUnreadCount, lastMessagesPerMessageBoxCount}, syncReason])` returns
`{"messageBoxes": [...], "hasNext": bool}`.

- **`activeOnly: true`** (what start-up uses) lists only the chats shown in the
  chat list: 39 boxes on the test account, `hasNext: false` with a limit of 1000.
- **`activeOnly: false`** also lists inactive chats: 388 boxes on the same
  account (188 `U…` users, 159 `C…` groups, 2 `R…` rooms). These are hidden or
  old chats. They have `lastMessages: []` and `unreadCount: "0"`.
- **`lastDeliveredMessageId`** is an object
  `{"messageId": "…", "deliveredTime": "…"}` (strings), in active and inactive
  boxes alike. `lastSeenMessageId` is a plain string. `unreadCount` is a string.
- **Paging:** `minChatId` set to the last box id of the previous page returns
  the next page, with no overlap (checked with pages of 5 against one page of
  10). Box order is neither by time nor alphabetical; the server keeps it
  stable between calls.

The TUI starts from the active list. Selecting the last chat fetches the next
page with `activeOnly: false` (`Client::message_boxes_page`), leaving out chats
it already lists. The new chats sort by `deliveredTime` and are fetched in full
when opened.

## Calls (call history only)

**Placing or answering calls is out of reach.** The extension registers no call
method at all (no `CallService`, `acquireCallRoute`, `acquireGroupCallRoute`;
see `src/endpoints.rs`), and the official apps' call media run on LINE's own
native VoIP stack, which neither `ltsm.wasm` nor the bundle contains. The bundle
only carries the enums: `ContentType.CALL = 6`, `OpType`
`NOTIFIED_RECEIVED_CALL = 50` / `CANCEL_CALL = 51`, TalkException codes
`NOT_ALLOWED_CALL = 37`, `INCOMPATIBLE_VOIP_VERSION = 39`,
`NOT_SUPPORT_CALL_SERVICE = 57`, and `Contact.capableVoiceCall/capableVideoCall`.
Calls were not probed live: a route request could ring the other side.

**Call log entries** are ordinary messages with `contentType` 6 that the server
adds to the chat. `CallRecord` (`src/models.rs`) parses them:

| Kind | Metadata | Status |
|---|---|---|
| 1:1 | `TYPE` (`A` voice / `V` video), `RESULT` (`INFO` = normal; `CANCELED`, `REJECTED`, `BUSY`, `NO_ANSWER`, …), `DURATION` (ms), `CAUSE`, `VERSION` | inferred (official apps / community clients) |
| Group | `GC_EVT_TYPE` (`S` started, `E` ended, `I` invited), `GC_MEDIA_TYPE` (`AUDIO`, `VIDEO`, `LIVE`), `GC_CHAT_MID`, `DURATION` (ms) | inferred |

Not live-verified: on 2026-11-14 the account's server-side history (recent
300 + older pages over 39 chats, ≈1 000 messages) held no `contentType` 6 message.
Parsing is therefore lenient: unknown `RESULT`/`GC_EVT_TYPE` values are kept
verbatim (`CallOutcome::Other`), a missing `TYPE` reads as voice, and a record
with talk time counts as answered. `cli watch` prints the raw metadata of call
messages so the first real one can confirm or correct this table. Ops 50/51 are
legacy and were never seen on the stream; the TUI only turns them into a
status-line hint ("… is calling — answer on your phone").

## Operations, read state, settings and stickers (live, 2026-11-14)

All read-only calls against the account, plus a replay of the operation
stream from 300 revisions back (`cli watch REV`, which now prints `param3`).

**Operation parameters** (decoded by `Operation::event()`, `src/ops.rs`):

| Op | Meaning | `param1` | `param2` | `param3` | Status |
|---|---|---|---|---|---|
| 2 `NOTIFIED_UPDATE_PROFILE` | a friend changed their profile | user | attribute (`2` name, `8`, `16` status) | JSON, e.g. `{"OLD_DISPLAY_NAME","DISPLAY_NAME"}` | live |
| 40 `SEND_CHAT_CHECKED` | **we** read a chat (any device) | chat | message id | `"0"` | live |
| 55 `NOTIFIED_READ_MESSAGE` | someone read a chat | chat — in 1:1 chats the reader's mid | reader | last read message id | live |
| 61 `NOTIFIED_LEAVE_CHAT` | member left | chat | member | message id | live |
| 62 `NOTIFIED_TYPING` | typing | chat (inferred) | user (inferred) | — | reference enum only; never observed |
| 64 `DESTROY_MESSAGE` | **we** unsent a message | chat | message id | `"0"` | live |
| 65 `NOTIFIED_DESTROY_MESSAGE` | someone unsent a message | chat | message id | `"0"` | live |
| 121 `UPDATE_CHAT` | we changed a chat | chat | `ChatAttribute` (`1` = name) | — | live |
| 128 `NOTIFIED_DELETE_SELF_FROM_CHAT` | member left a group | chat | member | — | live |
| 131 `REJECT_CHAT_INVITATION` | we declined (any device) | chat | — | — | live |
| 140 `NOTIFIED_SEND_REACTION` | reaction changed | message id | `{"chatMid", "curr": ReactionType}` (no `curr` = removed, inferred) | reactor | live |

Also seen: 43 and 48 with no params. LINE message ids are increasing
18-digit numbers, so read marks compare numerically (`ops::message_id_cmp`).

**Messages** from `getRecentMessagesV2` always carry `reactions`
(`[{fromUserMid, atMillis, reactionType: {predefinedReactionType} |
{paidReactionType: {productId, emojiId, resourceType, version}}}]`, live)
and `deliveredTime`. **Message boxes** carry `lastSeenMessageId` (our read
position) and `lastDeliveredMessageId`.

**Read ranges.** `getMessageReadRange([[chatIds], syncReason])` is served but
answered `[]` for 30 chats (6 user, 24 group) with sync reasons 1, 2, 3 and 4.
Read state therefore comes from op 55 as it arrives (and is kept in the
store); reads from before a client was listening are unknown. The reference
documents the result as `[{chatId, ranges: {mid: [{startMessageId,
endMessageId, startTime, endTime}]}}]`; `Client::message_read_ranges` parses that.

**Flex messages cannot be sent.** `sendMessage` with `contentType` 22
(`FLEX_JSON` + `ALT_TEXT` metadata, plain, before any sealing) fails with
HTTP 400, gateway 10051, TalkException 11 `INCOMPATIBLE_APP_VERSION`
("Incompatible app version"), to a sealed 1:1 chat and to our own mid
alike (live, 2026-11-14). The extension never sends Flex; it only renders
received ones (`FLEX_JSON` into a sandboxed iframe, `ALT_TEXT` in lists).
Flex is a LINE Official Account / Messaging API (bot) feature; personal
accounts receive Flex messages but cannot send them, so this is by design
rather than a Chrome-protocol gap.

**Typing.** The extension registers no method to send a typing
notification, and none of the reference's code sends or reads op 62.

**Methods the extension does not register are not served**: `findChatByTicket`
and `acceptChatInvitationByTicket` answer HTTP 404 with gateway code `10001`
`REQUEST_NOT_FOUND`. Joining a group by invitation link/QR is therefore not
possible over this protocol.

**Settings.** `getSettingsAttributes2([[attr]])` returns a whole `Settings`
struct with only the asked-for field filled (others at defaults), which
pins the `SettingsAttributeEx` numbers: `0` notificationEnable, `1`
notificationMuteExpiration (`"-1"` = not paused), `2` notificationNewMessage,
`3` notificationGroupInvitation, `4` notificationShowMessage, `40`
notificationMention, `45` notificationThumbnail, `67` notificationReaction
(live). `updateSettingsAttributes2([reqSeq, [attrs], {fields}])` is from the
reference; rune has not written settings live.

**Mute state.** `getChats` returns `notificationDisabled` on each chat; contacts
carry a `settings` bit mask (`ContactSetting`: `1` notification disable, `2`
name override, `4` hide, `8` favorite) (live). Changing it uses `updateChat`
with `updatedAttribute` `8` (NOTIFICATION_SETTING) for groups and
`updateContactSetting(reqSeq, mid, 1, "true"|"false")` for friends — shapes
from the reference enums, not exercised live.

**Owned stickers.** `ShopService.getOwnedProductSummaries(["stickershop",
offset, limit, {language, country}])` returns `{offset, totalSize,
productList: [{id, name, latestVersion, productTypeSummary: {stickerSummary:
{stickerResourceType, stickerIdRanges: [{start, size}]}}}]}` (live), so a
package's sticker ids are `start .. start + size` with no extra request.
The list includes LINE's free default packages (`1`–`4`, `11537`–`11539`
on the test account). Sending a sticker from a package you do not own is
rejected with HTTP 400 (reported live, 2026-11-14), so the TUI's "Recent"
tab only offers stickers whose `STKPKGID` is in this list.

**Animated stickers.** Sticker messages carry `STKOPT`: `"A"` on animated
ones (live; `"0"` or no key on static ones). The public sticker CDN serves
the animation at `stickershop/v1/sticker/{STKID}/android/sticker_animation.png`
(and `iPhone/sticker_animation@2x.png`) as an APNG (`acTL` chunk, live).

**Stream replay.** Replaying from 300 revisions back delivered 288
operations at once. A replay from 3 000 back delivered nothing within 40 s
(not investigated further); a client that falls that far behind should
re-read the chat list, which the TUI does on `fullSync`.

**Timed refresh.** `refreshApiRetryPolicy` and the token schedule come with
every `tokenV3IssueResult`; `Client::run_token_refresher` renews at
`tokenIssueTimeEpochSec + durationUntilRefreshInSec` minus a tenth of the
period (at most 10 min), re-checking the wall clock every minute. The
reference has an opt-in timer that fires 0–1 s before the same moment.

## Not implemented yet

E-mail
login, shop/buddy endpoints (reachable through `Client::call`), timeline
features beyond covers (the gateway serves no other timeline path).
