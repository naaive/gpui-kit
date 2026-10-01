// Reads the system's own network tools: interface addresses from `ipconfig`,
// `ip` or `ifconfig`, and listening ports from `netstat` and `tasklist` on
// Windows or `lsof` elsewhere. The parsers are plain functions of the
// commands' output; `run` is only called from async code, inside `try`.
import { run } from "process";
import { environment } from "launcher/api";

export function platform() {
  try {
    return environment().platform ?? "windows";
  } catch (_) {
    return "windows";
  }
}

async function output(command, args) {
  const result = await run(command, args);
  if (result.code !== 0 && !result.stdout.trim()) {
    throw new Error(result.stderr.trim() || `${command} exited with code ${result.code}`);
  }
  return result.stdout;
}

// MARK: Interfaces

function cleanAddress(text) {
  return text.replace(/\(.*?\)/g, "").trim();
}

/** `[{ adapter, family, address }]` from Windows `ipconfig`. */
export function parseIpconfig(text) {
  const addresses = [];
  let adapter = null;
  for (const line of text.split(/\r?\n/)) {
    if (line.trim() === "") continue;
    if (!/^\s/.test(line)) {
      // "Ethernet adapter Ethernet:" names the block below it.
      adapter = line.trim().replace(/:$/, "").replace(/^.*? adapter /i, "");
      continue;
    }
    const match = /^\s+([^:]*?)[ .]*:\s*(.+)$/.exec(line);
    if (!match || !adapter) continue;
    const [, label, value] = match;
    const family = /IPv4/i.test(label) ? "IPv4" : /IPv6/i.test(label) ? "IPv6" : null;
    if (!family) continue;
    const address = cleanAddress(value);
    if (address) addresses.push({ adapter, family, address, label: label.trim() });
  }
  return addresses;
}

/** `[{ adapter, family, address }]` from `ifconfig` (macOS, BSD, older Linux). */
export function parseIfconfig(text) {
  const addresses = [];
  let adapter = null;
  for (const line of text.split(/\r?\n/)) {
    const header = /^([^\s:]+):?\s/.exec(line);
    if (header && !/^\s/.test(line)) adapter = header[1].replace(/:$/, "");
    const inet = /^\s+inet6?\s+(?:addr:)?([0-9a-fA-F.:%a-zA-Z]+)/.exec(line);
    if (inet && adapter) {
      const family = /inet6/.test(line) ? "IPv6" : "IPv4";
      addresses.push({ adapter, family, address: inet[1], label: family });
    }
  }
  return addresses;
}

/** `[{ adapter, family, address }]` from `ip -o addr show` (Linux). */
export function parseIpAddr(text) {
  return text
    .split(/\r?\n/)
    .map((line) => /^\d+:\s+(\S+)\s+(inet6?)\s+(\S+)/.exec(line))
    .filter(Boolean)
    .map(([, adapter, inet, cidr]) => ({
      adapter,
      family: inet === "inet6" ? "IPv6" : "IPv4",
      address: cidr.split("/")[0],
      label: inet === "inet6" ? "IPv6" : "IPv4",
    }));
}

function isLoopback({ address }) {
  return address === "::1" || address.startsWith("127.");
}

/** The local interface addresses, loopback left out. */
export async function localAddresses() {
  const system = platform();
  if (system === "windows") return parseIpconfig(await output("ipconfig", [])).filter((each) => !isLoopback(each));
  if (system === "linux") {
    try {
      return parseIpAddr(await output("ip", ["-o", "addr", "show"])).filter((each) => !isLoopback(each));
    } catch (_) {
      // Fall through to ifconfig on systems without `ip`.
    }
  }
  return parseIfconfig(await output("ifconfig", [])).filter((each) => !isLoopback(each));
}

// MARK: Ports

function splitAddress(text) {
  const at = text.lastIndexOf(":");
  return { host: text.slice(0, at), port: Number(text.slice(at + 1)) };
}

/** Listening TCP sockets from Windows `netstat -ano`: `[{ port, pid, address }]`. */
export function parseNetstat(text) {
  const sockets = [];
  for (const line of text.split(/\r?\n/)) {
    const parts = line.trim().split(/\s+/);
    if (parts[0] !== "TCP" || parts.length < 5) continue;
    const [, local, foreign] = parts;
    // A listening socket has no remote end; the state word is localized.
    if (!/^(0\.0\.0\.0|\[::\]|\*):0$/.test(foreign)) continue;
    const { host, port } = splitAddress(local);
    const pid = Number(parts[parts.length - 1]);
    if (Number.isFinite(port) && Number.isFinite(pid)) sockets.push({ port, pid, address: host });
  }
  return sockets;
}

/** PID to image name from `tasklist /fo csv /nh`. */
export function parseTasklist(text) {
  const names = new Map();
  for (const line of text.split(/\r?\n/)) {
    const fields = [...line.matchAll(/"([^"]*)"/g)].map((match) => match[1]);
    if (fields.length >= 2) names.set(Number(fields[1]), fields[0]);
  }
  return names;
}

/** Listening sockets from `lsof -iTCP -sTCP:LISTEN -P -n`: `[{ port, pid, address, process }]`. */
export function parseLsof(text) {
  const sockets = [];
  for (const line of text.split(/\r?\n/).slice(1)) {
    const parts = line.trim().split(/\s+/);
    if (parts.length < 9) continue;
    const name = parts[parts.length - 1] === "(LISTEN)" ? parts[parts.length - 2] : parts[parts.length - 1];
    const { host, port } = splitAddress(name);
    const pid = Number(parts[1]);
    if (Number.isFinite(port) && Number.isFinite(pid)) {
      sockets.push({ port, pid, address: host, process: parts[0].replace(/\\x20/g, " ") });
    }
  }
  return sockets;
}

/** One entry per port and process, with every address it listens on. */
export function groupSockets(sockets, names = new Map()) {
  const groups = new Map();
  for (const socket of sockets) {
    const key = `${socket.port}-${socket.pid}`;
    const group = groups.get(key) ?? {
      port: socket.port,
      pid: socket.pid,
      process: socket.process ?? names.get(socket.pid) ?? "Unknown",
      addresses: [],
    };
    if (!group.addresses.includes(socket.address)) group.addresses.push(socket.address);
    groups.set(key, group);
  }
  return [...groups.values()].sort((a, b) => a.port - b.port || a.pid - b.pid);
}

export async function listeningPorts() {
  if (platform() === "windows") {
    const sockets = parseNetstat(await output("netstat", ["-ano", "-p", "TCP"]).catch(() => ""))
      .concat(parseNetstat(await output("netstat", ["-ano", "-p", "TCPv6"]).catch(() => "")));
    let names = new Map();
    try {
      names = parseTasklist(await output("tasklist", ["/fo", "csv", "/nh"]));
    } catch (_) {
      // Ports without names are still worth showing.
    }
    if (sockets.length === 0) {
      // Ask once more without a protocol filter, and let a failure surface.
      return groupSockets(parseNetstat(await output("netstat", ["-ano"])), names);
    }
    return groupSockets(sockets, names);
  }
  return groupSockets(parseLsof(await output("lsof", ["-iTCP", "-sTCP:LISTEN", "-P", "-n"])));
}

/** Ends a process; throws with what the system said when it cannot. */
export async function killProcess(pid) {
  const result =
    platform() === "windows"
      ? await run("taskkill", ["/PID", String(pid), "/F"])
      : await run("kill", ["-9", String(pid)]);
  if (result.code !== 0) throw new Error(result.stderr.trim() || result.stdout.trim() || `Exit code ${result.code}`);
}
