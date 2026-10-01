// Hashes the typed or selected text with MD5, SHA-1 and the SHA-2 family,
// as hex (Enter copies it) or Base64. An algorithm the runtime lacks is
// left out rather than failing the page.
import { createHash } from "crypto";
import { Action } from "launcher";
import { resultItem, TextToolView } from "../lib/ui.js";

const ALGORITHMS = [
  ["md5", "MD5"],
  ["sha1", "SHA-1"],
  ["sha256", "SHA-256"],
  ["sha384", "SHA-384"],
  ["sha512", "SHA-512"],
];

function digest(algorithm, text) {
  try {
    const hex = String(createHash(algorithm).update(text).digest("hex"));
    const base64 = String(createHash(algorithm).update(text).digest("base64"));
    return { hex, base64 };
  } catch (_) {
    return null;
  }
}

export default class HashText extends TextToolView {
  page() {
    return {
      placeholder: "Text to hash…",
      empty_title: "Hash Text",
      empty_description: "Type text, or select some before opening the launcher.",
      showing_detail: true,
    };
  }

  rows(input) {
    if (input === "") return [];
    return ALGORITHMS.map(([algorithm, title]) => [title, digest(algorithm, input)])
      .filter(([, result]) => result !== null)
      .map(([title, { hex, base64 }]) =>
        resultItem(title, title, hex, {
          icon: "hash",
          accessory: `${hex.length * 4} bits`,
          extra: [new Action("Copy as Base64").icon("binary").shortcut("secondary-shift-c").copy(base64)],
        }),
      );
  }
}
