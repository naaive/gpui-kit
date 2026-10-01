# Network Tools

Your IP addresses, the ports processes listen on, and DNS records.

## Commands

- **My IP Address**: your public IPv4 and IPv6 addresses, the approximate
  location and network of the public address, and every local interface
  address (loopback is left out). Each part loads separately, so if one fails
  the others still show. Refresh with Ctrl/Cmd-R.
- **Search Ports**: every listening TCP port, with the process and PID. Search
  by port, process name or PID. Copy PID, open `http://localhost:<port>`, or
  end the process with Kill Process, which asks first. Ending a system or
  elevated process on Windows needs the launcher to run as administrator.
- **DNS Lookup**: type a domain, URL or e-mail address to see its A, AAAA,
  CNAME, MX, TXT and NS records. Queries use DNS over HTTPS through Cloudflare,
  or Google Public DNS if Cloudflare cannot be reached.

## Permissions

- Programs: `ipconfig`, `netstat`, `tasklist` and `taskkill` on Windows;
  `ifconfig`, `ip`, `lsof` and `kill` on macOS and Linux. They read your
  network configuration and listening ports, and `taskkill` or `kill` runs only
  when you confirm Kill Process.
- Network (GET only): `api.ipify.org/` and `api6.ipify.org/` (public address),
  `ipinfo.io/json` (location), `cloudflare-dns.com/dns-query` and
  `dns.google/resolve` (DNS). None of them needs an account or key.
