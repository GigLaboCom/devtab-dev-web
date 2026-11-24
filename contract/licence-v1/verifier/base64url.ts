/**
 * Strict base64url (RFC 4648 §5) as the licence token uses it: the alphabet `A–Z a–z 0–9 - _` only, no `=` padding,
 * no spaces or line breaks, and a length that is not 1 modulo 4. Anything else is not a token's part.
 */
const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';

const VALUE = (() => {
  const table = new Int8Array(128).fill(-1);
  for (let i = 0; i < ALPHABET.length; i += 1) table[ALPHABET.charCodeAt(i)] = i;
  return table;
})();

/** Strict base64url without padding; `undefined` for anything else. */
export const base64urlDecode = (text: string): Uint8Array<ArrayBuffer> | undefined => {
  if (typeof text !== 'string' || text.length % 4 === 1) return undefined;
  const out = new Uint8Array((text.length * 3) >> 2);
  let bits = 0;
  let count = 0;
  let at = 0;
  for (let i = 0; i < text.length; i += 1) {
    const code = text.charCodeAt(i);
    const value = code < 128 ? VALUE[code]! : -1;
    if (value < 0) return undefined;
    bits = ((bits << 6) | value) & 0xffff;
    count += 6;
    if (count >= 8) {
      count -= 8;
      out[at] = (bits >> count) & 0xff;
      at += 1;
    }
  }
  return out;
};

export const base64urlEncode = (bytes: Uint8Array): string => {
  let text = '';
  let i = 0;
  for (; i + 2 < bytes.length; i += 3) {
    const n = (bytes[i]! << 16) | (bytes[i + 1]! << 8) | bytes[i + 2]!;
    text += ALPHABET[n >> 18]! + ALPHABET[(n >> 12) & 63]! + ALPHABET[(n >> 6) & 63]! + ALPHABET[n & 63]!;
  }
  if (bytes.length - i === 1) {
    const n = bytes[i]! << 16;
    text += ALPHABET[n >> 18]! + ALPHABET[(n >> 12) & 63]!;
  } else if (bytes.length - i === 2) {
    const n = (bytes[i]! << 16) | (bytes[i + 1]! << 8);
    text += ALPHABET[n >> 18]! + ALPHABET[(n >> 12) & 63]! + ALPHABET[(n >> 6) & 63]!;
  }
  return text;
};
