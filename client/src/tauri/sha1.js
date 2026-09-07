// Synchronous SHA-1 over a UTF-8 string, returning a lowercase hex digest.
// (window.crypto.subtle is async, which doesn't fit the commit-tree code path.)
export function sha1hex (str) {
  const bytes = new TextEncoder().encode(str)

  const ml = bytes.length
  const withOne = ml + 1
  const totalLen = Math.ceil((withOne + 8) / 64) * 64
  const msg = new Uint8Array(totalLen)
  msg.set(bytes)
  msg[ml] = 0x80
  const view = new DataView(msg.buffer)
  const bitLen = ml * 8
  view.setUint32(totalLen - 8, Math.floor(bitLen / 0x100000000), false)
  view.setUint32(totalLen - 4, bitLen >>> 0, false)

  let h0 = 0x67452301 | 0
  let h1 = 0xEFCDAB89 | 0
  let h2 = 0x98BADCFE | 0
  let h3 = 0x10325476 | 0
  let h4 = 0xC3D2E1F0 | 0

  const w = new Int32Array(80)
  const rotl = (n, b) => (n << b) | (n >>> (32 - b))

  for (let i = 0; i < totalLen; i += 64) {
    for (let j = 0; j < 16; j++) {
      w[j] = view.getInt32(i + j * 4, false)
    }
    for (let j = 16; j < 80; j++) {
      w[j] = rotl(w[j - 3] ^ w[j - 8] ^ w[j - 14] ^ w[j - 16], 1)
    }

    let a = h0; let b = h1; let c = h2; let d = h3; let e = h4

    for (let j = 0; j < 80; j++) {
      let f, k
      if (j < 20) {
        f = (b & c) | (~b & d)
        k = 0x5A827999
      } else if (j < 40) {
        f = b ^ c ^ d
        k = 0x6ED9EBA1
      } else if (j < 60) {
        f = (b & c) | (b & d) | (c & d)
        k = 0x8F1BBCDC
      } else {
        f = b ^ c ^ d
        k = 0xCA62C1D6
      }
      const temp = (rotl(a, 5) + f + e + k + w[j]) | 0
      e = d
      d = c
      c = rotl(b, 30)
      b = a
      a = temp
    }

    h0 = (h0 + a) | 0
    h1 = (h1 + b) | 0
    h2 = (h2 + c) | 0
    h3 = (h3 + d) | 0
    h4 = (h4 + e) | 0
  }

  const toHex = (n) => (n >>> 0).toString(16).padStart(8, '0')
  return toHex(h0) + toHex(h1) + toHex(h2) + toHex(h3) + toHex(h4)
}
