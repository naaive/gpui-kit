// Text encodings: Base64 (and its URL-safe form), URL encoding, HTML
// entities, and reading a JWT. Each decoder answers null for input it cannot
// decode, so the command shows only the rows that make sense.
import { Buffer } from "buffer";

export function base64Encode(text) {
  return Buffer.from(text, "utf8").toString("base64");
}

export function base64UrlEncode(text) {
  return base64Encode(text).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** The bytes of standard or URL-safe Base64, or null when it is not Base64. */
export function base64Bytes(text) {
  const compact = text.replace(/\s+/g, "");
  if (compact.length < 2 || !/^[A-Za-z0-9+/_-]+={0,2}$/.test(compact)) return null;
  const standard = compact.replace(/-/g, "+").replace(/_/g, "/").replace(/=+$/, "");
  if (standard.length % 4 === 1) return null;
  const padded = standard + "=".repeat((4 - (standard.length % 4)) % 4);
  return Buffer.from(padded, "base64");
}

/** Decoded Base64 as text, or null when it is not Base64 or not UTF-8 text. */
export function base64Decode(text) {
  const bytes = base64Bytes(text);
  if (!bytes || bytes.length === 0) return null;
  const decoded = bytes.toString("utf8");
  // A replacement character means the bytes were not UTF-8: binary data.
  if (decoded.includes("�")) return null;
  // Text made mostly of control characters is binary too.
  const controls = decoded.match(/[\u0000-\u0008\u000E-\u001F]/g)?.length ?? 0;
  return controls > decoded.length / 10 ? null : decoded;
}

export function urlEncode(text) {
  return encodeURIComponent(text);
}

export function urlDecode(text) {
  if (!/%[0-9a-fA-F]{2}|\+/.test(text)) return null;
  try {
    return decodeURIComponent(text.replace(/\+/g, " "));
  } catch (_) {
    return null;
  }
}

const ENTITIES = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };
const NAMED = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  nbsp: " ",
  copy: "©",
  reg: "®",
  trade: "™",
  hellip: "…",
  mdash: "—",
  ndash: "–",
  laquo: "«",
  raquo: "»",
  ldquo: "“",
  rdquo: "”",
  lsquo: "‘",
  rsquo: "’",
  euro: "€",
  pound: "£",
  yen: "¥",
  cent: "¢",
  deg: "°",
  times: "×",
  divide: "÷",
  middot: "·",
  bull: "•",
  sect: "§",
  para: "¶",
};

export function htmlEncode(text) {
  return text.replace(/[&<>"']/g, (char) => ENTITIES[char]);
}

export function htmlDecode(text) {
  if (!/&(#\d+|#x[0-9a-fA-F]+|[a-zA-Z]+);/.test(text)) return null;
  return text.replace(/&(#\d+|#x[0-9a-fA-F]+|[a-zA-Z]+);/g, (whole, body) => {
    if (body[0] === "#") {
      const code = body[1] === "x" || body[1] === "X" ? parseInt(body.slice(2), 16) : parseInt(body.slice(1), 10);
      return code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : whole;
    }
    return NAMED[body] ?? whole;
  });
}

function jsonPart(part) {
  const bytes = base64Bytes(part);
  if (!bytes) return null;
  try {
    const value = JSON.parse(bytes.toString("utf8"));
    return value && typeof value === "object" ? value : null;
  } catch (_) {
    return null;
  }
}

/** `{ header, payload, signature }` of a JWT, or null when it is not one. */
export function decodeJwt(text) {
  const parts = text.trim().replace(/^Bearer\s+/i, "").split(".");
  if (parts.length !== 3) return null;
  const header = jsonPart(parts[0]);
  const payload = jsonPart(parts[1]);
  if (!header || !payload) return null;
  return { header, payload, signature: parts[2] };
}

/** How a JWT time claim (Unix seconds) reads: the date, and how far from now. */
export function describeClaimTime(seconds, now = Date.now()) {
  if (typeof seconds !== "number") return null;
  const date = new Date(seconds * 1000);
  return { iso: date.toISOString(), relative: relativeTime(date.getTime() - now) };
}

export function relativeTime(milliseconds) {
  const future = milliseconds > 0;
  let amount = Math.abs(milliseconds) / 1000;
  const units = [
    ["second", 60],
    ["minute", 60],
    ["hour", 24],
    ["day", 30],
    ["month", 12],
    ["year", Infinity],
  ];
  for (const [unit, size] of units) {
    if (amount < size) {
      const whole = Math.round(amount);
      const label = `${whole} ${unit}${whole === 1 ? "" : "s"}`;
      return future ? `in ${label}` : `${label} ago`;
    }
    amount /= size;
  }
  return "";
}
