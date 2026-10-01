// Lists the TCP ports processes listen on, searchable by port, process name
// or PID, with the process's PID to copy and, after asking, a way to end it.
import { View } from "gpui-kit";
import { Action, ActionPanel, ActionPanelSection, List, ListItem } from "launcher";
import { show_toast } from "launcher/api";
import { killProcess, listeningPorts, platform } from "../lib/system.js";

const WELL_KNOWN = {
  22: "SSH",
  53: "DNS",
  80: "HTTP",
  135: "RPC",
  139: "NetBIOS",
  443: "HTTPS",
  445: "SMB",
  1433: "SQL Server",
  3000: "Dev server",
  3306: "MySQL",
  3389: "Remote Desktop",
  5173: "Vite",
  5432: "PostgreSQL",
  5900: "VNC",
  6379: "Redis",
  8080: "HTTP alternate",
  27017: "MongoDB",
};

function addressLabel(address) {
  if (address === "0.0.0.0" || address === "[::]" || address === "*") return "All interfaces";
  if (address === "127.0.0.1" || address === "[::1]") return "Localhost";
  return address;
}

export default class SearchPorts extends View {
  init(_props, cx) {
    this.ports = [];
    this.error = null;
    this.loading = true;
    this.reload(cx);
  }

  reload(cx) {
    this.loading = true;
    cx.spawn(async (task) => {
      try {
        this.ports = await listeningPorts();
        this.error = null;
      } catch (error) {
        this.ports = [];
        this.error = String(error?.message ?? error);
      } finally {
        this.loading = false;
        task.notify();
      }
    });
  }

  kill(entry, cx) {
    cx.spawn(async (task) => {
      try {
        await killProcess(entry.pid);
        show_toast({ title: `Ended ${entry.process}`, message: `PID ${entry.pid}`, style: "success" });
        this.reload(task);
      } catch (error) {
        const elevated = platform() === "windows" ? "Run the launcher as administrator to end it." : "It may belong to another user.";
        show_toast({
          title: `Cannot end ${entry.process}`,
          message: `${String(error?.message ?? error)} ${elevated}`,
          style: "failure",
        });
      }
      task.notify();
    });
  }

  row(entry) {
    const service = WELL_KNOWN[entry.port];
    const where = entry.addresses.map(addressLabel).filter((label, index, all) => all.indexOf(label) === index);
    const url = `http://localhost:${entry.port}`;
    const item = new ListItem(`${entry.port}-${entry.pid}`, String(entry.port))
      .icon("ethernet-port")
      .subtitle(entry.process)
      .keyword(entry.process)
      .keyword(String(entry.pid))
      .accessory(where.join(", "))
      .accessory_tooltip(entry.addresses.join(", "))
      .tag(`PID ${entry.pid}`, "neutral");
    return (service ? item.keyword(service).tag(service, "accent") : item).actions(
      new ActionPanel().children([
        new Action("Copy PID").icon("copy").copy(String(entry.pid)),
        new Action("Copy Port").icon("copy").copy(String(entry.port)),
        new Action("Open in Browser").icon("arrow-up-right").shortcut("secondary-o").open_url(url),
        new Action("Copy Process Name").icon("copy").shortcut("secondary-shift-c").copy(entry.process),
        new Action("Refresh")
          .icon("refresh-cw")
          .shortcut("secondary-r")
          .run((cx) => {
            this.reload(cx);
            cx.notify();
          }),
        new ActionPanelSection("Danger Zone").children([
          new Action("Kill Process…")
            .icon("skull")
            .shortcut("secondary-shift-k")
            .destructive()
            .confirm(`End ${entry.process} (PID ${entry.pid})?`, `It stops listening on port ${entry.port} and loses unsaved work.`)
            .run((cx) => this.kill(entry, cx)),
        ]),
      ]),
    );
  }

  render() {
    const [title, description] = this.error
      ? ["Cannot list ports", this.error]
      : this.ports.length === 0
        ? ["No listening ports", "No process is listening on a TCP port."]
        : ["No matching port", "Search by port, process name or PID."];
    return new List()
      .placeholder("Search by port, process or PID…")
      .loading(this.loading)
      .empty_title(this.loading ? "Reading ports…" : title)
      .empty_description(this.loading ? "" : description)
      .children(this.ports.map((entry) => this.row(entry)));
  }
}
