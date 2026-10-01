// What Network Tools asks the internet: the public address (ipify), where it
// is (ipinfo.io), and DNS records over HTTPS (Cloudflare, then Google when
// Cloudflare cannot be reached). None of them needs a key.

async function getJson(url, headers = {}) {
  const response = await fetch(url, { headers: { Accept: "application/json", ...headers } });
  if (!response.ok) throw new Error(`${new URL(url).host} answered ${response.status}`);
  return JSON.parse(await response.text());
}

/** The public IPv4 address. */
export async function publicIpv4() {
  const { ip } = await getJson("https://api.ipify.org/?format=json");
  return String(ip);
}

/** The public IPv6 address; fails on a network without IPv6. */
export async function publicIpv6() {
  const { ip } = await getJson("https://api6.ipify.org/?format=json");
  return String(ip);
}

/** `{ ip, city, region, country, org, loc, timezone }` of the public address. */
export async function location() {
  return getJson("https://ipinfo.io/json");
}

export const RECORD_TYPES = [
  ["A", 1],
  ["AAAA", 28],
  ["CNAME", 5],
  ["MX", 15],
  ["TXT", 16],
  ["NS", 2],
];

const TYPE_NAMES = new Map([...RECORD_TYPES.map(([name, code]) => [code, name]), [6, "SOA"], [65, "HTTPS"]]);

const RESOLVERS = [
  { name: "Cloudflare", url: (name, type) => `https://cloudflare-dns.com/dns-query?name=${encodeURIComponent(name)}&type=${type}`, headers: { Accept: "application/dns-json" } },
  { name: "Google", url: (name, type) => `https://dns.google/resolve?name=${encodeURIComponent(name)}&type=${type}`, headers: {} },
];

const STATUS = { 1: "Format error", 2: "Server failure", 3: "No such domain", 5: "Refused" };

/** Answers of one type: `{ status, answers: [{ type, name, ttl, data }] }`. */
async function query(resolver, name, type) {
  const body = await getJson(resolver.url(name, type), resolver.headers);
  return {
    status: body.Status ?? 0,
    answers: (body.Answer ?? []).map((answer) => {
      const answered = TYPE_NAMES.get(answer.type) ?? String(answer.type);
      const data = String(answer.data ?? "");
      return {
        type: answered,
        name: String(answer.name ?? "").replace(/\.$/, ""),
        ttl: answer.TTL,
        // TXT strings come quoted; host names come fully qualified, with a final dot.
        data: answered === "TXT" ? data.replace(/^"|"$/g, "") : data.replace(/\.$/, ""),
      };
    }),
  };
}

/**
 * Every record type for `name`, asked in parallel: `{ resolver, records,
 * error }`, where `records` maps a type to its answers. The first resolver
 * that answers at all is used for every type.
 */
export async function lookup(name) {
  let lastError = null;
  for (const resolver of RESOLVERS) {
    try {
      const results = await Promise.all(RECORD_TYPES.map(([type]) => query(resolver, name, type)));
      const nxdomain = results.find((result) => result.status !== 0);
      const records = {};
      RECORD_TYPES.forEach(([type], index) => {
        // Only answers of the asked type: an A query also lists the CNAME chain.
        records[type] = results[index].answers.filter((answer) => answer.type === type);
      });
      return { resolver: resolver.name, records, error: nxdomain ? STATUS[nxdomain.status] ?? `DNS status ${nxdomain.status}` : null };
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError ?? new Error("No DNS resolver answered");
}

/** The host name in what the user typed: a domain, URL or e-mail address. */
export function domainOf(text) {
  let value = text.trim().toLowerCase();
  value = value.replace(/^[a-z][a-z0-9+.-]*:\/\//, "").replace(/^[^@/]*@/, "");
  value = value.split(/[/?#]/)[0].replace(/:\d+$/, "").replace(/\.$/, "");
  return /^[a-z0-9_]([a-z0-9_-]*[a-z0-9_])?(\.[a-z0-9_]([a-z0-9_-]*[a-z0-9_])?)*\.?[a-z0-9-]*$/i.test(value) && value.includes(".")
    ? value
    : null;
}
