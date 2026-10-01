// Encodes and decodes the typed or selected text: Base64, URL encoding and
// HTML entities both ways, and a JWT's header and payload. Decoders show a
// row only when the input decodes.
import {
  Action,
  ActionPanel,
  Detail,
  ListItem,
  ListSection,
  MetadataLabel,
  MetadataSeparator,
  MetadataTags,
} from "launcher";
import {
  base64Decode,
  base64Encode,
  base64UrlEncode,
  decodeJwt,
  describeClaimTime,
  htmlDecode,
  htmlEncode,
  urlDecode,
  urlEncode,
} from "../lib/codecs.js";
import { codeBlock, ResultView, resultItem, TextToolView } from "../lib/ui.js";

function jwtSection(token) {
  const { header, payload, signature } = token;
  const now = Date.now();
  const expires = describeClaimTime(payload.exp, now);
  const issued = describeClaimTime(payload.iat, now);
  const notBefore = describeClaimTime(payload.nbf, now);
  const expired = typeof payload.exp === "number" && payload.exp * 1000 < now;
  const status = expires ? (expired ? ["Expired", "danger"] : ["Valid", "success"]) : ["No Expiry", "neutral"];
  const headerJson = JSON.stringify(header, null, 2);
  const payloadJson = JSON.stringify(payload, null, 2);
  const metadata = [
    new MetadataTags("Status").tag(status[0], status[1]),
    new MetadataLabel("Algorithm", String(header.alg ?? "Unknown")),
    ...(expires ? [new MetadataLabel("Expires", `${expires.iso} (${expires.relative})`)] : []),
    ...(issued ? [new MetadataLabel("Issued", `${issued.iso} (${issued.relative})`)] : []),
    ...(notBefore ? [new MetadataLabel("Not Before", `${notBefore.iso} (${notBefore.relative})`)] : []),
    ...(payload.sub !== undefined ? [new MetadataLabel("Subject", String(payload.sub))] : []),
    ...(payload.iss !== undefined ? [new MetadataLabel("Issuer", String(payload.iss))] : []),
    new MetadataSeparator(),
    new MetadataLabel("Signature", "Not verified"),
  ];
  const markdown = `## Header\n\n${codeBlock(headerJson, "json")}\n\n## Payload\n\n${codeBlock(payloadJson, "json")}`;
  const row = (id, title, value, subtitle) =>
    new ListItem(id, title)
      .icon("key-round")
      .subtitle(subtitle)
      .tag(status[0], status[1])
      .detail(new Detail(markdown).children(metadata))
      .actions(
        new ActionPanel().children([
          new Action(`Copy ${title}`).icon("copy").copy(value),
          new Action("Show Token")
            .icon("maximize-2")
            .shortcut("secondary-y")
            .push(
              () =>
                new ResultView({
                  title: "JWT",
                  value: `// Header\n${headerJson}\n\n// Payload\n${payloadJson}`,
                  language: "json",
                  metadata: [
                    ["Algorithm", String(header.alg ?? "Unknown")],
                    ...(expires ? [["Expires", `${expires.iso} (${expires.relative})`]] : []),
                  ],
                }),
              "JWT",
            ),
          new Action("Copy Header").icon("copy").copy(headerJson),
          new Action("Copy Signature").copy(signature),
        ]),
      );
  return new ListSection("JWT").children([
    row("jwt-payload", "Payload", payloadJson, expires ? `Expires ${expires.relative}` : "Decoded payload"),
    row("jwt-header", "Header", headerJson, `${header.alg ?? "?"} · ${header.typ ?? "JWT"}`),
  ]);
}

export default class EncodeDecode extends TextToolView {
  page() {
    return {
      placeholder: "Text to encode or decode…",
      empty_title: "Encode or Decode Text",
      empty_description: "Type text, a Base64 string, a URL-encoded string or a JWT, or select some first.",
      showing_detail: true,
    };
  }

  rows(input) {
    if (input === "") return [];
    const sections = [];
    const token = decodeJwt(input);
    if (token) sections.push(jwtSection(token));

    const decoded = [
      ["base64-decode", "Base64 Decoded", base64Decode(input), "binary"],
      ["url-decode", "URL Decoded", urlDecode(input), "link"],
      ["html-decode", "HTML Entities Decoded", htmlDecode(input), "code"],
    ].filter(([, , value]) => value !== null && value !== input);
    if (decoded.length > 0 && !token) {
      sections.push(
        new ListSection("Decode").children(
          decoded.map(([id, title, value, icon]) => resultItem(id, title, value, { icon })),
        ),
      );
    }

    sections.push(
      new ListSection("Encode").children([
        resultItem("base64", "Base64", base64Encode(input), { icon: "binary" }),
        resultItem("base64url", "Base64 (URL-Safe)", base64UrlEncode(input), { icon: "binary" }),
        resultItem("url", "URL Encoded", urlEncode(input), { icon: "link" }),
        resultItem("html", "HTML Entities", htmlEncode(input), { icon: "code" }),
      ]),
    );
    return sections;
  }
}
