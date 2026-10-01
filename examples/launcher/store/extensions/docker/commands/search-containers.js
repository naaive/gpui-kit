// Lists Docker containers, running and stopped, with actions to start, stop,
// restart or remove one, read its logs, or open a port it publishes. When
// docker is missing or its daemon is down, the list says so instead.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  ActionPanelSubmenu,
  Detail,
  List,
  ListDropdown,
  ListDropdownItem,
  ListItem,
  ListSection,
  MetadataLabel,
  MetadataLink,
  MetadataSeparator,
  MetadataTags,
} from "launcher";
import { show_toast } from "launcher/api";
import { describe, docker, invoke, parseLines } from "../lib/docker.js";
import { ContainerLogs } from "../lib/logs.js";

const STATE_TONES = {
  running: "success",
  paused: "warning",
  restarting: "warning",
  created: "accent",
  removing: "warning",
  dead: "danger",
};

/** `0.0.0.0:8080->80/tcp, [::]:8080->80/tcp` → `[{ host: 8080, container: "80/tcp" }]`, deduplicated. */
function publishedPorts(ports) {
  const found = new Map();
  for (const match of String(ports ?? "").matchAll(/(?:[\d.]+|\[[^\]]*\]|::):(\d+)->(\d+(?:-\d+)?)\/(tcp|udp)/g)) {
    const [, host, container, protocol] = match;
    if (!found.has(host)) found.set(host, { host: Number(host), container: `${container}/${protocol}`, protocol });
  }
  return [...found.values()];
}

function toContainer(raw) {
  const state = String(raw.State ?? "").toLowerCase() || (/^Up/.test(raw.Status ?? "") ? "running" : "exited");
  return {
    id: raw.ID,
    short: String(raw.ID ?? "").slice(0, 12),
    name: String(raw.Names ?? raw.ID).split(",")[0],
    image: raw.Image ?? "",
    command: String(raw.Command ?? "").replace(/^"|"$/g, ""),
    state,
    status: raw.Status ?? state,
    created: raw.CreatedAt ?? "",
    ports: publishedPorts(raw.Ports),
    rawPorts: raw.Ports ?? "",
    networks: String(raw.Networks ?? "").split(",").filter(Boolean),
  };
}

function detail(container) {
  const metadata = [
    new MetadataTags("State").tag(container.state, STATE_TONES[container.state] ?? "neutral"),
    new MetadataLabel("Status", container.status),
    new MetadataLabel("Image", container.image),
    new MetadataLabel("ID", container.short),
    new MetadataLabel("Created", container.created.replace(/ [+-]\d{4} \w+$/, "") || "Unknown"),
  ];
  if (container.networks.length > 0) {
    metadata.push(container.networks.reduce((tags, network) => tags.tag(network), new MetadataTags("Networks")));
  }
  if (container.ports.length > 0) {
    metadata.push(new MetadataSeparator());
    for (const port of container.ports) {
      const url = `http://localhost:${port.host}`;
      metadata.push(new MetadataLink(`Port ${port.container}`, `localhost:${port.host}`, url));
    }
  } else if (container.rawPorts) {
    metadata.push(new MetadataLabel("Ports", container.rawPorts));
  }
  const command = container.command ? `\n\n\`\`\`sh\n${container.command}\n\`\`\`` : "";
  return new Detail(`# ${container.name.replace(/_/g, "\\_")}${command}`).children(metadata);
}

export default class SearchContainers extends View {
  init(_props, cx) {
    this.cx = cx;
    this.containers = [];
    this.filter = "all";
    this.loading = true;
    this.error = null;
    this.reload();
  }

  reload() {
    this.loading = true;
    this.cx.spawn(async (task) => {
      try {
        const output = await docker(["ps", "--all", "--no-trunc", "--format", "{{json .}}"]);
        this.containers = parseLines(output).map(toContainer);
        this.error = null;
      } catch (error) {
        this.containers = [];
        this.error = describe(error);
      } finally {
        this.loading = false;
        task.notify();
      }
    });
  }

  /** Runs `docker <args>` for one container, with a toast before and after. */
  perform(container, progress, done, args) {
    this.cx.spawn(async (task) => {
      const id = `docker-${container.id}`;
      show_toast({ id, title: `${progress} ${container.name}…`, style: "progress" });
      try {
        await invoke(args);
        show_toast({ id, title: `${done} ${container.name}`, style: "success" });
      } catch (error) {
        const [title, message] = describe(error);
        show_toast({ id, title, message, style: "failure" });
      }
      this.reload();
      task.notify();
    });
  }

  lifecycle(container) {
    const running = container.state === "running" || container.state === "restarting";
    if (container.state === "paused") {
      return [new Action("Unpause Container").icon("circle-play").run(() => this.perform(container, "Unpausing", "Unpaused", ["unpause", container.id]))];
    }
    if (!running) {
      return [new Action("Start Container").icon("play").run(() => this.perform(container, "Starting", "Started", ["start", container.id]))];
    }
    return [
      new Action("Stop Container").icon("circle-stop").shortcut("secondary-shift-s").run(() => this.perform(container, "Stopping", "Stopped", ["stop", container.id])),
      new Action("Restart Container").icon("rotate-cw").shortcut("secondary-shift-r").run(() => this.perform(container, "Restarting", "Restarted", ["restart", container.id])),
    ];
  }

  browser(container) {
    const ports = container.ports.filter((port) => port.protocol === "tcp");
    if (ports.length === 0) return [];
    if (ports.length === 1) {
      return [new Action("Open in Browser").icon("globe").shortcut("secondary-o").open_url(`http://localhost:${ports[0].host}`)];
    }
    return [
      new ActionPanelSubmenu("Open in Browser").icon("globe").shortcut("secondary-o").children(
        ports.map((port) => new Action(`localhost:${port.host} → ${port.container}`).open_url(`http://localhost:${port.host}`)),
      ),
    ];
  }

  row(container) {
    const running = container.state === "running";
    const base = new ListItem(container.id, container.name).icon("container");
    const item = (running ? base.icon_tone("success") : base)
      .subtitle(container.image)
      .keyword(container.image)
      .keyword(container.short);
    const ports = container.ports.map((port) => `:${port.host}`).join(" ");
    return (ports ? item.accessory(ports).accessory_tooltip("Published ports") : item)
      .tag(container.state, STATE_TONES[container.state] ?? "neutral")
      .detail(detail(container))
      .actions(
        new ActionPanel().children([
          ...this.lifecycle(container),
          new Action("View Logs")
            .icon("scroll-text")
            .shortcut("secondary-l")
            .push(() => new ContainerLogs({ id: container.id, name: container.name }), `Logs of ${container.name}`),
          ...this.browser(container),
          new ActionPanelSection("Copy").children([
            new Action("Copy Container ID").icon("copy").shortcut("secondary-shift-c").copy(container.id),
            new Action("Copy Container Name").icon("copy").copy(container.name),
            new Action("Copy Image Name").icon("copy").copy(container.image),
          ]),
          new ActionPanelSection("Manage").children([
            new Action("Refresh").icon("refresh-cw").shortcut("secondary-r").run((cx) => {
              this.reload();
              cx.notify();
            }),
            new Action("Remove Container")
              .icon("trash")
              .shortcut("secondary-shift-x")
              .destructive()
              .confirm(
                `Remove ${container.name}?`,
                running ? "The container is running; it will be stopped and removed. Its writable layer is lost." : "The container and its writable layer are removed.",
              )
              .run(() => this.perform(container, "Removing", "Removed", ["rm", ...(running ? ["--force"] : []), container.id])),
          ]),
        ]),
      );
  }

  sections() {
    const shown = this.containers.filter((container) => {
      if (this.filter === "running") return container.state === "running";
      if (this.filter === "stopped") return container.state !== "running";
      return true;
    });
    const running = shown.filter((container) => container.state === "running");
    const other = shown.filter((container) => container.state !== "running");
    return [
      ["Running", running],
      ["Stopped", other],
    ]
      .filter(([, containers]) => containers.length > 0)
      .map(([title, containers]) =>
        new ListSection(title).subtitle(String(containers.length)).children(containers.map((container) => this.row(container))),
      );
  }

  render() {
    const [title, description] = this.error
      ? this.error
      : this.containers.length === 0
        ? ["No Containers", "Containers you create with `docker run` or Compose show here."]
        : ["No Matching Containers", "Try another name, image or ID."];
    return new List()
      .placeholder("Search containers by name, image or ID…")
      .loading(this.loading)
      .showing_detail(this.containers.length > 0)
      .empty_title(this.loading && this.containers.length === 0 ? "Asking Docker…" : title)
      .empty_description(this.loading && this.containers.length === 0 ? "Listing containers." : description)
      .dropdown(
        new ListDropdown("Filter Containers")
          .value(this.filter)
          .children([
            new ListDropdownItem("all", "All Containers"),
            new ListDropdownItem("running", "Running"),
            new ListDropdownItem("stopped", "Stopped"),
          ])
          .on_change((value, cx) => {
            this.filter = value;
            cx.notify();
          }),
      )
      .children(this.sections());
  }
}
