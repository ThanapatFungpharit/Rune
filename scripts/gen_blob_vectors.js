// Media-blob encryption vectors from the extension's own algorithm, run on
// WebCrypto exactly as ltsmSandbox.js does (`mv` HKDF, `gv` AES-CTR + HMAC,
// `bv` 128 KiB chunk hashes for video).
//   node scripts/gen_blob_vectors.js > tests/fixtures/blob_vectors.json
'use strict';
const {subtle} = require('crypto').webcrypto;
const crypto = require('crypto');

async function mv(km) {
    const k = await subtle.importKey('raw', km, 'HKDF', false, ['deriveBits']);
    const r = new Uint8Array(await subtle.deriveBits({
        name: 'HKDF',
        hash: 'SHA-256',
        salt: new ArrayBuffer(0),
        info: new TextEncoder().encode('FileEncryption')
    }, k, 608));
    return {encKey: r.slice(0, 32), macKey: r.slice(32, 64), nonce: r.slice(64)};
}

async function hmac(data, key) {
    const k = await subtle.importKey('raw', key, {name: 'HMAC', hash: 'SHA-256'}, false, ['sign']);
    return new Uint8Array(await subtle.sign('HMAC', k, data));
}

async function bv(buf) {
    const out = [];
    for (let i = 0; i < buf.byteLength; i += 131072) out.push(new Uint8Array(await subtle.digest('SHA-256', buf.slice(i, i + 131072))));
    return out;
}

const concat = (arrs) => {
    const n = arrs.reduce((a, b) => a + b.length, 0), o = new Uint8Array(n);
    let p = 0;
    for (const a of arrs) {
        o.set(a, p);
        p += a.length;
    }
    return o;
};

async function gv(data, km, chunked) {
    const {encKey, macKey, nonce} = await mv(km);
    const counter = new Uint8Array([...nonce, 0, 0, 0, 0]);
    const k = await subtle.importKey('raw', encKey, 'AES-CTR', false, ['encrypt']);
    const ct = new Uint8Array(await subtle.encrypt({name: 'AES-CTR', counter, length: 32}, k, data));
    if (chunked) {
        const h = await bv(ct.buffer);
        return {blob: concat([ct, await hmac(concat(h), macKey)]), hashes: concat(h)};
    }
    return {blob: concat([ct, await hmac(ct, macKey)])};
}

(async () => {
    const cases = [];
    for (const [len, chunked] of [[0, false], [1, false], [1000, false], [300000, false], [300000, true], [131072, true]]) {
        const km = crypto.createHash('sha256').update(`km-${len}-${chunked}`).digest();
        const data = Buffer.alloc(len, 0).map((_, i) => (i * 7 + 3) % 251);
        const {blob, hashes} = await gv(data, km, chunked);
        cases.push({
            keyMaterial: km.toString('base64'), len, chunked,
            blobSha256: crypto.createHash('sha256').update(blob).digest('hex'), blobLen: blob.length,
            hashesSha256: hashes ? crypto.createHash('sha256').update(hashes).digest('hex') : null
        });
    }
    process.stdout.write(JSON.stringify(cases, null, 2) + '\n');
})();
