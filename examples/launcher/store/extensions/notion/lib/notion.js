// Notion's REST API with an internal integration token, and the helpers that
// turn its pages, databases and blocks into what the commands show. The
// integration only sees pages that were shared with it.
import { launch } from "launcher/api";

const API = "https://api.notion.com/v1";
const VERSION = "2022-06-28";
/** Notion refuses rich text longer than this in one piece. */
const TEXT_LIMIT = 2000;
/** And more blocks than this in one request. */
const BLOCK_LIMIT = 100;

export class NotionError extends Error {}

function token() {
  return String(launch().preferences.token ?? "").trim();
}

function errorFrom(status, answer) {
  const code = answer?.code ?? "";
  if (status === 401 || code === "unauthorized") {
    return new NotionError("Notion refused the integration token. Check it in the extension's preferences.");
  }
  if (status === 404 || code === "object_not_found") {
    return new NotionError("Notion cannot find that page. Share it with your integration (••• → Connections).");
  }
  if (status === 403 || code === "restricted_resource") {
    return new NotionError("The integration may not do that. Give it the capability in notion.so/my-integrations.");
  }
  if (status === 429) return new NotionError("Notion's rate limit was reached; try again shortly.");
  return new NotionError(answer?.message ?? `Notion answered ${status}.`);
}

/** Calls the API and answers the parsed JSON; throws a readable `NotionError`. */
export async function notion(method, path, body = null) {
  let response;
  try {
    const options = {
      method,
      headers: { Authorization: `Bearer ${token()}`, "Notion-Version": VERSION, Accept: "application/json" },
    };
    if (body !== null) {
      options.headers["Content-Type"] = "application/json";
      options.body = JSON.stringify(body);
    }
    response = await fetch(`${API}${path}`, options);
  } catch (error) {
    throw new NotionError(`Cannot reach Notion: ${String(error?.message ?? error)}`);
  }
  let answer = null;
  try {
    answer = JSON.parse(await response.text());
  } catch {
    // Reported by status below.
  }
  if (!response.ok) throw errorFrom(response.status, answer);
  return answer;
}

/** Pages and databases whose title matches `query`, most recently edited first. */
export async function search(query, { object = null, page_size = 25 } = {}) {
  const body = { page_size, sort: { direction: "descending", timestamp: "last_edited_time" } };
  if (query) body.query = query;
  if (object) body.filter = { property: "object", value: object };
  const answer = await notion("POST", "/search", body);
  return (answer?.results ?? []).filter((result) => !result.archived && !result.in_trash);
}

export function plainText(richText) {
  return (richText ?? []).map((part) => part.plain_text ?? part.text?.content ?? "").join("");
}

/** The title of a page or database. */
export function titleOf(object) {
  if (object.object === "database") return plainText(object.title) || "Untitled";
  const title = Object.values(object.properties ?? {}).find((property) => property.type === "title");
  return plainText(title?.title) || "Untitled";
}

/** The name of a database's title property, which a new row must fill in. */
export function titlePropertyOf(database) {
  return Object.entries(database.properties ?? {}).find(([, property]) => property.type === "title")?.[0] ?? "Name";
}

/** `{ emoji }` or `{ image }` for a page's icon, or `null`. */
export function iconOf(object) {
  const icon = object.icon;
  if (!icon) return null;
  if (icon.type === "emoji") return { emoji: icon.emoji };
  if (icon.type === "external") return { image: icon.external?.url ?? null };
  if (icon.type === "file") return { image: icon.file?.url ?? null };
  if (icon.type === "custom_emoji") return { image: icon.custom_emoji?.url ?? null };
  return null;
}

/** The `notion://` link that opens the page in the desktop app. */
export function appUrl(url) {
  return String(url).replace(/^https:\/\//, "notion://");
}

function propertyText(property) {
  switch (property.type) {
    case "title":
      return plainText(property.title);
    case "rich_text":
      return plainText(property.rich_text);
    case "number":
      return property.number === null ? "" : String(property.number);
    case "select":
      return property.select?.name ?? "";
    case "status":
      return property.status?.name ?? "";
    case "multi_select":
      return (property.multi_select ?? []).map((option) => option.name).join(", ");
    case "date":
      return property.date ? [property.date.start, property.date.end].filter(Boolean).join(" → ") : "";
    case "checkbox":
      return property.checkbox ? "Yes" : "No";
    case "url":
      return property.url ?? "";
    case "email":
      return property.email ?? "";
    case "phone_number":
      return property.phone_number ?? "";
    case "people":
      return (property.people ?? []).map((person) => person.name ?? "Someone").join(", ");
    case "relation":
      return property.relation?.length ? `${property.relation.length} linked` : "";
    case "files":
      return (property.files ?? []).map((file) => file.name).join(", ");
    case "formula": {
      const value = property.formula?.[property.formula?.type];
      return value === null || value === undefined ? "" : typeof value === "object" ? value.start ?? "" : String(value);
    }
    case "rollup":
      return property.rollup?.type === "number" && property.rollup.number !== null ? String(property.rollup.number) : "";
    case "created_time":
      return String(property.created_time ?? "").slice(0, 10);
    case "last_edited_time":
      return String(property.last_edited_time ?? "").slice(0, 10);
    case "created_by":
    case "last_edited_by":
      return property[property.type]?.name ?? "";
    case "unique_id":
      return property.unique_id ? `${property.unique_id.prefix ? `${property.unique_id.prefix}-` : ""}${property.unique_id.number}` : "";
    default:
      return "";
  }
}

/** A page's properties other than its title, as `[name, text, type]`, the empty ones left out. */
export function propertiesOf(page) {
  return Object.entries(page.properties ?? {})
    .filter(([, property]) => property.type !== "title")
    .map(([name, property]) => [name, propertyText(property), property.type, property])
    .filter(([, text]) => text !== "");
}

/** Markdown for the first blocks of a page: enough to recognize it beside the list. */
export async function pagePreview(id) {
  const answer = await notion("GET", `/blocks/${id}/children?page_size=40`);
  const lines = [];
  for (const block of answer?.results ?? []) {
    const content = block[block.type] ?? {};
    const text = plainText(content.rich_text);
    switch (block.type) {
      case "heading_1":
        lines.push(`## ${text}`);
        break;
      case "heading_2":
        lines.push(`### ${text}`);
        break;
      case "heading_3":
        lines.push(`#### ${text}`);
        break;
      case "bulleted_list_item":
        lines.push(`- ${text}`);
        break;
      case "numbered_list_item":
        lines.push(`1. ${text}`);
        break;
      case "to_do":
        lines.push(`- [${content.checked ? "x" : " "}] ${text}`);
        break;
      case "quote":
      case "callout":
        lines.push(`> ${text}`);
        break;
      case "code":
        lines.push(`\`\`\`${content.language ?? ""}\n${text}\n\`\`\``);
        break;
      case "divider":
        lines.push("---");
        break;
      case "child_page":
        lines.push(`- 📄 ${content.title ?? "Untitled"}`);
        break;
      case "child_database":
        lines.push(`- 🗃 ${content.title ?? "Untitled"}`);
        break;
      default:
        if (text) lines.push(text);
    }
  }
  return lines.join("\n\n");
}

/** Paragraph blocks for plain text: one per line, long lines split where Notion requires. */
export function paragraphs(text) {
  const blocks = [];
  for (const line of String(text ?? "").split(/\r?\n/)) {
    for (let start = 0; start < Math.max(line.length, 1); start += TEXT_LIMIT) {
      const piece = line.slice(start, start + TEXT_LIMIT);
      blocks.push({
        object: "block",
        type: "paragraph",
        paragraph: { rich_text: piece ? [{ type: "text", text: { content: piece } }] : [] },
      });
    }
  }
  // Drop blank lines at either end.
  while (blocks.length && blocks[0].paragraph.rich_text.length === 0) blocks.shift();
  while (blocks.length && blocks[blocks.length - 1].paragraph.rich_text.length === 0) blocks.pop();
  return blocks;
}

/** Creates a page under a page or a database (`parent` is a search result) and answers it. */
export async function createPage(parent, title, text) {
  const blocks = paragraphs(text);
  const titleText = [{ type: "text", text: { content: title.slice(0, TEXT_LIMIT) } }];
  const body =
    parent.object === "database"
      ? { parent: { database_id: parent.id }, properties: { [titlePropertyOf(parent)]: { title: titleText } } }
      : { parent: { page_id: parent.id }, properties: { title: { title: titleText } } };
  body.children = blocks.slice(0, BLOCK_LIMIT);
  const page = await notion("POST", "/pages", body);
  await appendBlocks(page.id, blocks.slice(BLOCK_LIMIT));
  return page;
}

/** Adds blocks at the end of a page, a hundred at a time. */
export async function appendBlocks(id, blocks) {
  for (let start = 0; start < blocks.length; start += BLOCK_LIMIT) {
    await notion("PATCH", `/blocks/${id}/children`, { children: blocks.slice(start, start + BLOCK_LIMIT) });
  }
}

/** A user-facing message for anything the requests may throw. */
export function describe(error) {
  if (error instanceof NotionError) return error.message;
  const text = String(error?.message ?? error);
  return /not allow|capabilit|permission/i.test(text)
    ? "The extension may not reach Notion; allow its network access."
    : `Cannot reach Notion: ${text}`;
}
