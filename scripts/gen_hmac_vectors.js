// Generate X-Hmac test vectors with the *reference* implementation: OkLine's
// Node bridge (ltsm_bridge.js + ltsmSandbox.js + ltsm.wasm from LINE Chrome
// 3.7.2). Used to check that rune's native wasmtime host is byte-compatible.
//
//   node scripts/gen_hmac_vectors.js <path/to/OkLine/okline/ltsm> > tests/fixtures/hmac_vectors.json
'use strict';
const { spawn } = require('child_process');
const path = require('path');
const readline = require('readline');

const dir = process.argv[2];
if (!dir) { console.error('usage: gen_hmac_vectors.js <okline/ltsm dir>'); process.exit(2); }

const cases = [
  ['', '/api/talk/thrift/Talk/TalkService/getProfile', '[2]'],
  ['', '/api/talk/thrift/LoginQrCode/SecondaryQrCodeLoginService/createSession', '[{}]'],
  ['eyJhbGciOiJIUzI1NiJ9.test.token', '/api/operation/receive?version=3.7.2&localRev=5', ''],
  ['eyJhbGciOiJIUzI1NiJ9.test.token', '/api/talk/thrift/Talk/TalkService/sendMessage',
   '[0,{"to":"u0123456789abcdef0123456789abcdef","toType":0,"text":"hi","contentType":0,"contentMetadata":{}}]'],
  ['another-token', '/api/talk/thrift/Talk/TalkService/sendMessage',
   '[1,{"to":"c0","text":"สวัสดี 👋 日本語","contentMetadata":{}}]'],
  ['another-token', '/api/operation/receive?version=3.7.2&language=en_US&lastPartialFullSyncs=%7B%7D&localRev=123456789', ''],
  ['x'.repeat(300), '/api/auth/tokenRefresh', '{"refreshToken":"' + 'r'.repeat(200) + '"}'],
  ['another-token', '/api/talk/thrift/Talk/TalkService/getContactsV2', JSON.stringify([{ targetUserMids: Array.from({ length: 40 }, (_, i) => 'u' + String(i).padStart(32, '0')) }])],
];

const child = spawn(process.execPath, [path.join(dir, 'ltsm_bridge.js')], { stdio: ['pipe', 'pipe', 'inherit'] });
const rl = readline.createInterface({ input: child.stdout });
const pending = new Map();
let ready;
const readyP = new Promise((r) => { ready = r; });
rl.on('line', (line) => {
  const m = JSON.parse(line);
  if ('ready' in m) return ready(m.ready);
  const p = pending.get(m.id); pending.delete(m.id);
  m.error ? p.reject(new Error(m.error)) : p.resolve(m.result);
});
let id = 0;
const call = (req) => new Promise((resolve, reject) => {
  req.id = ++id; pending.set(req.id, { resolve, reject });
  child.stdin.write(JSON.stringify(req) + '\n');
});

(async () => {
  if (!(await readyP)) throw new Error('bridge failed to start');
  const out = [];
  for (const [accessToken, p, body] of cases) {
    const hmac = await call({ op: 'hmac', accessToken, path: p, body });
    out.push({ accessToken, path: p, body, hmac });
  }
  process.stdout.write(JSON.stringify(out, null, 2) + '\n');
  child.kill();
})().catch((e) => { console.error(e); child.kill(); process.exit(1); });
