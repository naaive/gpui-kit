// Looks up a domain's A, AAAA, CNAME, MX, TXT and NS records over DNS over
// HTTPS as the user types. A pause in typing starts the lookup, and an answer
// that arrives after a newer query is dropped.
import { View } from "gpui-kit";
import { Action, ActionPanel, List, ListItem, ListSection } from "launcher";
import { launch } from "launcher/api";
import { domainOf, lookup, RECORD_TYPES } from "../lib/web.js";

const TYPING_PAUSE_MS = 400;
const ICONS = { A: "server", AAAA: "server", CNAME: "link", MX: "mail", TXT: "text-quote", NS: "earth" };

function formatTtl(seconds) {
  if (typeof seconds !== "number") return "";
  if (seconds < 120) return `${seconds}s`;
  if (seconds < 7200) return `${Math.round(seconds / 60)}m`;
  if (seconds < 172800) return `${Math.round(seconds / 3600)}h`;
  return `${Math.round(seconds / 86400)}d`;
}

export default class DnsLookup extends View {
  init(_props, cx) {
    this.result = null;
    this.error = null;
    this.loading = false;
    this.generation = 0;
    this.pending = null;
    this.domain = domainOf(launch().arguments.domain ?? "");
    this.typed_text = launch().arguments.domain ?? "";
    if (this.domain) this.resolve(cx);
  }

  typed(text, cx) {
    this.typed_text = text;
    this.domain = domainOf(text);
    this.pending?.cancel();
    this.generation += 1;
    this.result = null;
    this.error = null;
    this.loading = this.domain !== null;
    this.pending = this.domain ? cx.timer.after(TYPING_PAUSE_MS, (timer) => this.resolve(timer)) : null;
    cx.notify();
  }

  resolve(cx) {
    const run = ++this.generation;
    const domain = this.domain;
    this.loading = true;
    cx.spawn(async (task) => {
      try {
        const result = await lookup(domain);
        if (run !== this.generation) return;
        this.result = result;
        this.error = null;
      } catch (error) {
        if (run !== this.generation) return;
        this.result = null;
        this.error = String(error?.message ?? error);
      } finally {
        if (run === this.generation) {
          this.loading = false;
          task.notify();
        }
      }
    });
  }

  sections() {
    const { records, resolver } = this.result;
    const all = RECORD_TYPES.flatMap(([type]) => records[type].map((record) => `${type}\t${record.ttl}\t${record.data}`)).join("\n");
    return RECORD_TYPES.filter(([type]) => records[type].length > 0).map(([type]) =>
      new ListSection(type).subtitle(`${records[type].length} via ${resolver}`).children(
        records[type].map((record, index) =>
          new ListItem(`${type}-${index}-${record.data}`, record.data)
            .icon(ICONS[type])
            .subtitle(record.name)
            .accessory(`TTL ${formatTtl(record.ttl)}`)
            .actions(
              new ActionPanel().children([
                new Action("Copy Value").icon("copy").copy(record.data),
                new Action("Copy All Records").icon("copy").shortcut("secondary-shift-c").copy(all),
                new Action("Open Domain in Browser").icon("arrow-up-right").open_url(`https://${this.domain}`),
              ]),
            ),
        ),
      ),
    );
  }

  render() {
    const list = new List()
      .placeholder("Domain, such as example.com…")
      .loading(this.loading)
      .on_query_change((text, cx) => this.typed(text, cx));
    if (this.loading) return list.empty_title("Looking up…").empty_description(`Asking for the records of ${this.domain}`);
    if (!this.domain) {
      return list
        .empty_title(this.typed_text.trim() ? "Not a domain name" : "Look Up DNS Records")
        .empty_description("Type a domain, such as example.com, to see its A, AAAA, CNAME, MX, TXT and NS records.");
    }
    if (this.error) return list.empty_title("Cannot look up records").empty_description(this.error);
    if (!this.result) return list.empty_title("Look Up DNS Records").empty_description("Keep typing…");
    const sections = this.sections();
    return list
      .empty_title(this.result.error ?? "No records")
      .empty_description(this.result.error ? `${this.domain} does not exist in DNS.` : `${this.domain} has no A, AAAA, CNAME, MX, TXT or NS records.`)
      .children(sections);
  }
}
