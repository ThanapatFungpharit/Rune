//! LINE's secure module (`ltsm.wasm`), run natively through wasmtime.
//!
//! The Chrome extension computes the mandatory `X-Hmac` request signature,
//! the QR-login Curve25519 key and all Letter Sealing (E2EE) primitives inside
//! this C++/Emscripten module. Rather than re-deriving LINE's custom key
//! derivation, we execute the *same* module and drive it the way
//! `ltsmSandbox.js` does (see `runtime.rs` for the import shims).
//!
//! Signing (from the sandbox's `get_hmac` handler):
//!
//! ```text
//! tokenKey = SecureKey.loadToken(<per-origin LTSM token>)
//! key      = tokenKey.deriveKey(SHA256("3.7.2"), SHA256(accessToken))
//! X-Hmac   = base64(new Hmac(key).digest(utf8(path + body)))
//! ```

mod runtime;

use std::collections::HashMap;
use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use sha2::{Digest, Sha256};

use runtime::{Arg, EmVal, Object, Ret, Runtime};

/// The `ltsm.wasm` shipped with LINE Chrome 3.7.2
/// (sha256 `58bb4e189ab9bbd7d72ed415258da36afd50e306cb019bab75329d4f5f1b65b3`).
pub const LTSM_WASM: &[u8] = include_bytes!("../../assets/ltsm.wasm");

/// Extension origin the module is told it runs under.
pub const DEFAULT_ORIGIN: &str = "chrome-extension://ophjlpahpchlmihnnnihgmmeilfjmjjc";

/// Per-origin LTSM tokens, copied from `ltsmSandbox.js` (3.7.2).
const ORIGIN_TOKENS: &[(&str, &str)] = &[
    (
        "chrome-extension://ophjlpahpchlmihnnnihgmmeilfjmjjc",
        "wODdrvWqmdP4Zliay-iF3cz3KZcK0ekrial868apg06TXeCo7A1hIQO0ESElHg6D",
    ),
    (
        "chrome-extension://jeafcjlknhfnfkfeliiolabnpjahldie",
        "GS-30Ed0WxiXR50y9hED4O1qmJ2QaBK0dFpc_w9ZaBSisb2rlnKvPUkMK_93GS30",
    ),
];

/// The extension version mixed into the HMAC key derivation.
pub const SIGNING_VERSION: &str = "3.7.2";

/// Error raised by the secure module (or by driving it incorrectly).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LtsmError(pub String);

impl fmt::Display for LtsmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ltsm: {}", self.0)
    }
}

impl std::error::Error for LtsmError {}

impl From<wasmtime::Error> for LtsmError {
    fn from(e: wasmtime::Error) -> Self {
        // `{:#}` includes the trap context (e.g. the C++ exception message).
        LtsmError(format!("{e:#}"))
    }
}

pub type Result<T> = std::result::Result<T, LtsmError>;

/// Handle to a key object (Curve25519 key, E2EE key or group key) held
/// inside the module. Handles are only valid for the [`Ltsm`] that issued them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyHandle(u32);

/// Handle to an ECDH channel (`E2EEChannel`) held inside the module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChannelHandle(u32);

struct Classes {
    secure_key: u32,
    hmac: u32,
    curve_key: u32,
    e2ee_key: u32,
}

/// A running instance of `ltsm.wasm`.
///
/// Not `Sync`: wrap it in a mutex to share it. Creating one compiles the
/// module (a few hundred milliseconds in release builds), so reuse it.
pub struct Ltsm {
    rt: Runtime,
    classes: Classes,
    token_key: Object,
    version_hash: Vec<u8>,
    /// Last derived HMAC key: (SHA256(access token), derived key, Hmac).
    hmac_cache: Option<([u8; 32], Object, Object)>,
    keys: HashMap<u32, Object>,
    channels: HashMap<u32, Object>,
    next_handle: u32,
}

fn bytes_of(r: Ret) -> Result<Vec<u8>> {
    match r {
        Ret::Val(EmVal::Bytes(b)) => Ok(b),
        other => Err(LtsmError(format!("expected bytes, got {other:?}"))),
    }
}

fn obj_of(r: Ret) -> Result<Object> {
    match r {
        Ret::Obj(o) => Ok(o),
        Ret::Val(EmVal::Object(class, ptr)) => Ok(Object { class, ptr }),
        other => Err(LtsmError(format!("expected object, got {other:?}"))),
    }
}

fn num_of(r: Ret) -> Result<f64> {
    match r {
        Ret::Num(n) => Ok(n),
        Ret::BigInt(n) => Ok(n as f64),
        other => Err(LtsmError(format!("expected number, got {other:?}"))),
    }
}

impl fmt::Debug for Ltsm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ltsm")
            .field("keys", &self.keys.len())
            .field("channels", &self.channels.len())
            .finish_non_exhaustive()
    }
}

impl Ltsm {
    /// Load the bundled module under the default extension origin.
    pub fn new() -> Result<Self> {
        Self::with_origin(LTSM_WASM, DEFAULT_ORIGIN)
    }

    /// Load a specific `ltsm.wasm` build for one of the known origins.
    pub fn with_origin(wasm: &[u8], origin: &str) -> Result<Self> {
        let token = ORIGIN_TOKENS
            .iter()
            .find(|(o, _)| *o == origin)
            .map(|(_, t)| *t)
            .ok_or_else(|| LtsmError(format!("no LTSM token for origin {origin}")))?;
        let mut rt = Runtime::new(wasm, origin)?;
        match rt.call_function("isAvailable", &[])? {
            Ret::Num(n) if n != 0.0 => {}
            _ => return Err(LtsmError("module reports not available".into())),
        }
        let classes = Classes {
            secure_key: rt.class_by_name("SecureKey")?,
            hmac: rt.class_by_name("Hmac")?,
            curve_key: rt.class_by_name("Curve25519Key")?,
            e2ee_key: rt.class_by_name("E2EEKey")?,
        };
        let token_key =
            obj_of(rt.call_static(classes.secure_key, "loadToken", &[Arg::Str(token.to_owned())])?)?;
        Ok(Ltsm {
            rt,
            classes,
            token_key,
            version_hash: Sha256::digest(SIGNING_VERSION.as_bytes()).to_vec(),
            hmac_cache: None,
            keys: HashMap::new(),
            channels: HashMap::new(),
            next_handle: 0,
        })
    }

    /// Compute the `X-Hmac` header for a request.
    ///
    /// `path` must be exactly the transmitted path (including the query
    /// string for GETs) and `body` exactly the transmitted body (`""` when
    /// there is none). `access_token` is `""` before login.
    pub fn hmac(&mut self, access_token: &str, path: &str, body: &str) -> Result<String> {
        let token_hash: [u8; 32] = Sha256::digest(access_token.as_bytes()).into();
        let hmac = match &self.hmac_cache {
            Some((h, _, hmac)) if *h == token_hash => *hmac,
            _ => {
                if let Some((_, derived, hmac)) = self.hmac_cache.take() {
                    self.rt.delete(hmac)?;
                    self.rt.delete(derived)?;
                }
                let derived = obj_of(self.rt.call_method(
                    self.token_key,
                    "deriveKey",
                    &[
                        Arg::Bytes(self.version_hash.clone()),
                        Arg::Bytes(token_hash.to_vec()),
                    ],
                )?)?;
                let hmac = self.rt.construct(self.classes.hmac, &[Arg::Obj(derived)])?;
                self.hmac_cache = Some((token_hash, derived, hmac));
                hmac
            }
        };
        let mut msg = Vec::with_capacity(path.len() + body.len());
        msg.extend_from_slice(path.as_bytes());
        msg.extend_from_slice(body.as_bytes());
        let digest = bytes_of(self.rt.call_method(hmac, "digest", &[Arg::Bytes(msg)])?)?;
        Ok(B64.encode(digest))
    }

    // -- key / channel bookkeeping (mirrors the sandbox's pS / mS maps) --------

    fn add_key(&mut self, o: Object) -> KeyHandle {
        self.next_handle += 1;
        self.keys.insert(self.next_handle, o);
        KeyHandle(self.next_handle)
    }

    fn add_channel(&mut self, o: Object) -> ChannelHandle {
        self.next_handle += 1;
        self.channels.insert(self.next_handle, o);
        ChannelHandle(self.next_handle)
    }

    fn key(&self, h: KeyHandle) -> Result<Object> {
        self.keys
            .get(&h.0)
            .copied()
            .ok_or_else(|| LtsmError(format!("unknown key handle {}", h.0)))
    }

    fn channel(&self, h: ChannelHandle) -> Result<Object> {
        self.channels
            .get(&h.0)
            .copied()
            .ok_or_else(|| LtsmError(format!("unknown channel handle {}", h.0)))
    }

    fn e2ee_key(&self, h: KeyHandle) -> Result<Object> {
        let o = self.key(h)?;
        if o.class != self.classes.e2ee_key {
            return Err(LtsmError("not an E2EE key".into()));
        }
        Ok(o)
    }

    /// Free a key object.
    pub fn release_key(&mut self, h: KeyHandle) -> Result<()> {
        match self.keys.remove(&h.0) {
            Some(o) => Ok(self.rt.delete(o)?),
            None => Ok(()),
        }
    }

    /// Free a channel object.
    pub fn release_channel(&mut self, h: ChannelHandle) -> Result<()> {
        match self.channels.remove(&h.0) {
            Some(o) => Ok(self.rt.delete(o)?),
            None => Ok(()),
        }
    }

    // -- Curve25519 / E2EE keys -------------------------------------------------

    /// `Curve25519Key.generate(true)` — the QR-login keypair.
    pub fn generate_curve_key(&mut self) -> Result<KeyHandle> {
        let o = obj_of(
            self.rt
                .call_static(self.classes.curve_key, "generate", &[Arg::Bool(true)])?,
        )?;
        Ok(self.add_key(o))
    }

    /// Raw public key bytes of any key handle.
    pub fn public_key(&mut self, h: KeyHandle) -> Result<Vec<u8>> {
        let o = self.key(h)?;
        bytes_of(self.rt.call_method(o, "getPublicKey", &[])?)
    }

    /// The server-assigned key id of an E2EE key.
    pub fn key_id(&mut self, h: KeyHandle) -> Result<i32> {
        let o = self.e2ee_key(h)?;
        Ok(num_of(self.rt.call_method(o, "getKeyId", &[])?)? as i32)
    }

    /// Key format version of an E2EE key.
    pub fn key_version(&mut self, h: KeyHandle) -> Result<i32> {
        let o = self.e2ee_key(h)?;
        Ok(num_of(self.rt.call_method(o, "getVersion", &[])?)? as i32)
    }

    /// Serialize an E2EE key so it can be persisted and re-loaded with
    /// [`Ltsm::load_e2ee_key`]. The blob is sensitive key material.
    pub fn export_e2ee_key(&mut self, h: KeyHandle) -> Result<Vec<u8>> {
        let o = self.e2ee_key(h)?;
        bytes_of(self.rt.call_method(o, "exportKey", &[])?)
    }

    /// `E2EEKey.loadKey(blob)`.
    pub fn load_e2ee_key(&mut self, blob: &[u8]) -> Result<KeyHandle> {
        let o = obj_of(self.rt.call_static(
            self.classes.e2ee_key,
            "loadKey",
            &[Arg::Bytes(blob.to_vec())],
        )?)?;
        Ok(self.add_key(o))
    }

    /// ECDH channel between one of our keys and a peer public key.
    pub fn create_channel(&mut self, key: KeyHandle, peer_public_key: &[u8]) -> Result<ChannelHandle> {
        let o = self.key(key)?;
        let ch = obj_of(
            self.rt
                .call_method(o, "createChannel", &[Arg::Bytes(peer_public_key.to_vec())])?,
        )?;
        Ok(self.add_channel(ch))
    }

    /// Unwrap the encrypted keychain delivered by `qrCodeLoginV2`
    /// (`metaData.encryptedKeyChain`) into E2EE key handles.
    pub fn unwrap_keychain(&mut self, ch: ChannelHandle, encrypted: &[u8]) -> Result<Vec<KeyHandle>> {
        let c = self.channel(ch)?;
        let chain = obj_of(self.rt.call_method(
            c,
            "unwrapE2EEKeyChain",
            &[Arg::Bytes(encrypted.to_vec())],
        )?)?;
        let n = num_of(self.rt.call_method(chain, "size", &[])?)? as i64;
        let mut out = Vec::new();
        for i in 0..n {
            let k = obj_of(self.rt.call_method(chain, "get", &[Arg::Int(i)])?)?;
            out.push(self.add_key(k));
        }
        self.rt.delete(chain)?;
        Ok(out)
    }

    /// Letter Sealing V2 encryption.
    #[allow(clippy::too_many_arguments)]
    pub fn encrypt_v2(
        &mut self,
        ch: ChannelHandle,
        to: &str,
        from: &str,
        sender_key_id: u32,
        receiver_key_id: u32,
        content_type: u32,
        sequence_number: u64,
        plaintext: &[u8],
    ) -> Result<Vec<u8>> {
        let c = self.channel(ch)?;
        bytes_of(self.rt.call_method(
            c,
            "encryptV2",
            &[
                Arg::Str(to.to_owned()),
                Arg::Str(from.to_owned()),
                Arg::Int(sender_key_id.into()),
                Arg::Int(receiver_key_id.into()),
                Arg::Int(content_type.into()),
                Arg::Int(sequence_number as i64),
                Arg::Bytes(plaintext.to_vec()),
            ],
        )?)
    }

    /// Letter Sealing V2 decryption.
    #[allow(clippy::too_many_arguments)]
    pub fn decrypt_v2(
        &mut self,
        ch: ChannelHandle,
        to: &str,
        from: &str,
        sender_key_id: u32,
        receiver_key_id: u32,
        content_type: u32,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>> {
        let c = self.channel(ch)?;
        bytes_of(self.rt.call_method(
            c,
            "decryptV2",
            &[
                Arg::Str(to.to_owned()),
                Arg::Str(from.to_owned()),
                Arg::Int(sender_key_id.into()),
                Arg::Int(receiver_key_id.into()),
                Arg::Int(content_type.into()),
                Arg::Bytes(ciphertext.to_vec()),
            ],
        )?)
    }

    /// Letter Sealing V1 encryption (legacy format).
    pub fn encrypt_v1(&mut self, ch: ChannelHandle, plaintext: &[u8]) -> Result<Vec<u8>> {
        let c = self.channel(ch)?;
        bytes_of(
            self.rt
                .call_method(c, "encryptV1", &[Arg::Bytes(plaintext.to_vec())])?,
        )
    }

    /// Letter Sealing V1 decryption (legacy format).
    pub fn decrypt_v1(&mut self, ch: ChannelHandle, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let c = self.channel(ch)?;
        bytes_of(
            self.rt
                .call_method(c, "decryptV1", &[Arg::Bytes(ciphertext.to_vec())])?,
        )
    }

    /// Unwrap a group's encrypted shared key into a key handle.
    pub fn unwrap_group_shared_key(&mut self, ch: ChannelHandle, encrypted: &[u8]) -> Result<KeyHandle> {
        let c = self.channel(ch)?;
        let k = obj_of(
            self.rt
                .call_method(c, "unwrapGroupSharedKey", &[Arg::Bytes(encrypted.to_vec())])?,
        )?;
        Ok(self.add_key(k))
    }

    /// Wrap a group shared key for one member's channel.
    pub fn wrap_group_shared_key(&mut self, ch: ChannelHandle, key: KeyHandle) -> Result<Vec<u8>> {
        let c = self.channel(ch)?;
        let k = self.key(key)?;
        bytes_of(self.rt.call_method(c, "wrapGroupSharedKey", &[Arg::Obj(k)])?)
    }

    /// E-mail login device confirmation: hash key chain to send back with
    /// `confirmE2EELogin`.
    pub fn generate_hash_key_chain(&mut self, ch: ChannelHandle, encrypted_chain: &[u8]) -> Result<Vec<u8>> {
        let c = self.channel(ch)?;
        bytes_of(self.rt.call_method(
            c,
            "generateHashKeyChainToConfirmE2EE",
            &[Arg::Bytes(encrypted_chain.to_vec())],
        )?)
    }

    /// Wrap a plain Curve25519 key as an `E2EEKey`
    /// (`new E2EEKey(key, version, keyId, <bool>)` — parameter order confirmed
    /// via `getVersion`/`getKeyId`; the meaning of the bool is unknown).
    /// Such synthetic keys work for ECDH, V1 and V2 (V2 needs real mids, see
    /// docs/PROTOCOL.md), but the module refuses to export them ("illegal
    /// operation"); only keychain-unwrapped keys are exportable.
    #[doc(hidden)]
    pub fn make_e2ee_key(&mut self, key: KeyHandle, version: u32, key_id: u32) -> Result<KeyHandle> {
        let k = self.key(key)?;
        let o = self.rt.construct(
            self.classes.e2ee_key,
            &[
                Arg::Obj(k),
                Arg::Int(version.into()),
                Arg::Int(key_id.into()),
                Arg::Bool(true),
            ],
        )?;
        Ok(self.add_key(o))
    }

    #[doc(hidden)]
    pub fn describe(&self) -> String {
        self.rt.describe()
    }

    #[doc(hidden)]
    pub fn live_js_values(&self) -> usize {
        self.rt.store.data().live_handles()
    }
}
