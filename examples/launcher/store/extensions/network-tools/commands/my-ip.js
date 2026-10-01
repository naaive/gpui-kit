// Shows this computer's addresses: the public IPv4 and IPv6 addresses and
// roughly where they are, and every local interface address. Each part loads
// on its own, so one that fails says why without hiding the others.
import { View } from "gpui-kit";
import { Action, ActionPanel, List, ListItem, ListSection } from "launcher";
import { localAddresses } from "../lib/system.js";
import { location, publicIpv4, publicIpv6 } from "../lib/web.js";

function message(error) {
  return String(error?.message ?? error);
}

export default class MyIp extends View {
  init(_props, cx) {
    this.refresh(cx);
  }

  refresh(cx) {
    this.ipv4 = { loading: true };
    this.ipv6 = { loading: true };
    this.place = { loading: true };
    this.local = { loading: true };
    this.load(cx, "ipv4", publicIpv4);
    this.load(cx, "ipv6", publicIpv6);
    this.load(cx, "place", location);
    this.load(cx, "local", localAddresses);
  }

  load(cx, field, loader) {
    cx.spawn(async (task) => {
      try {
        this[field] = { value: await loader() };
      } catch (error) {
        this[field] = { error: message(error) };
      }
      task.notify();
    });
  }

  get loading() {
    return [this.ipv4, this.ipv6, this.place, this.local].some((part) => part.loading);
  }

  panel(primary, extra = []) {
    return new ActionPanel().children([
      ...primary,
      ...extra,
      new Action("Refresh")
        .icon("refresh-cw")
        .shortcut("secondary-r")
        .run((cx) => {
          this.refresh(cx);
          cx.notify();
        }),
    ]);
  }

  publicRow(id, title, part, unavailable) {
    const item = new ListItem(id, part.value ?? (part.loading ? "Looking up…" : "Not available")).icon("globe").subtitle(title);
    if (part.value) {
      return item.actions(this.panel([new Action("Copy Address").icon("copy").copy(part.value)]));
    }
    return (part.error ? item.accessory(unavailable).accessory_tooltip(part.error) : item).actions(this.panel([]));
  }

  placeRow() {
    const { value, error, loading } = this.place;
    if (!value) {
      const item = new ListItem("location", loading ? "Looking up…" : "Not available").icon("map-pin").subtitle("Location");
      return (error ? item.accessory("Unavailable").accessory_tooltip(error) : item).actions(this.panel([]));
    }
    const where = [value.city, value.region, value.country].filter(Boolean).join(", ") || "Unknown";
    const details = [
      ["City", value.city],
      ["Region", value.region],
      ["Country", value.country],
      ["Network", value.org],
      ["Time Zone", value.timezone],
      ["Coordinates", value.loc],
    ].filter(([, text]) => text);
    const summary = details.map(([label, text]) => `${label}: ${text}`).join("\n");
    const maps = value.loc ? [new Action("Open in Maps").icon("map").open_url(`https://www.openstreetmap.org/?mlat=${value.loc.split(",")[0]}&mlon=${value.loc.split(",")[1]}&zoom=10`)] : [];
    const item = new ListItem("location", where).icon("map-pin").subtitle("Location");
    return (value.org ? item.accessory(value.org) : item).actions(this.panel([new Action("Copy Location").icon("copy").copy(summary), ...maps]));
  }

  localRows() {
    const { value, error, loading } = this.local;
    if (!value) {
      const item = new ListItem("local", loading ? "Reading interfaces…" : "Not available").icon("network");
      return [(error ? item.subtitle(error) : item).actions(this.panel([]))];
    }
    if (value.length === 0) return [new ListItem("local", "No interface addresses").icon("network").actions(this.panel([]))];
    const all = value.map((each) => `${each.adapter}\t${each.family}\t${each.address}`).join("\n");
    return value.map((each, index) => {
      const kind = each.label.replace(/[ .]+$/, "");
      const item = new ListItem(`local-${index}-${each.address}`, each.address)
        .icon(each.family === "IPv4" ? "ethernet-port" : "network")
        .subtitle(each.adapter)
        .tag(each.family, each.family === "IPv4" ? "accent" : "neutral");
      return (kind === each.family ? item : item.accessory(kind)).keyword(kind).actions(
        this.panel(
          [new Action("Copy Address").icon("copy").copy(each.address.replace(/%.*$/, ""))],
          [new Action("Copy All Local Addresses").icon("copy").shortcut("secondary-shift-c").copy(all)],
        ),
      );
    });
  }

  render() {
    return new List()
      .placeholder("Filter addresses…")
      .loading(this.loading)
      .empty_title("No matching address")
      .children([
        new ListSection("Public").children([
          this.publicRow("ipv4", "Public IPv4", this.ipv4, "Unavailable"),
          this.publicRow("ipv6", "Public IPv6", this.ipv6, "No IPv6"),
          this.placeRow(),
        ]),
        new ListSection("Local").children(this.localRows()),
      ]);
  }
}
