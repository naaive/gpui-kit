// Converts between Unix timestamps (seconds, milliseconds, microseconds)
// and dates: ISO 8601 in UTC and with the local offset, local time, and how
// long ago. With no input it shows now, and keeps ticking.
import { relativeTime } from "../lib/codecs.js";
import { resultItem, TextToolView } from "../lib/ui.js";

function pad(number, width = 2) {
  return String(Math.abs(Math.trunc(number))).padStart(width, "0");
}

/** ISO 8601 in local time, with the offset spelled out. */
function localIso(date) {
  const offset = -date.getTimezoneOffset();
  const sign = offset >= 0 ? "+" : "-";
  return (
    `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}` +
    `T${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}` +
    `${sign}${pad(offset / 60)}:${pad(offset % 60)}`
  );
}

/** `{ date, read_as }` for the input, or null when it is not a time. */
export function parseTime(input, now = Date.now()) {
  const text = input.trim();
  if (text === "" || /^now$/i.test(text)) return { date: new Date(now), read_as: "Now" };
  if (/^-?\d+(\.\d+)?$/.test(text)) {
    const digits = text.replace(/^-/, "").split(".")[0].length;
    const number = Number(text);
    if (digits <= 11) return { date: new Date(number * 1000), read_as: "Unix seconds" };
    if (digits <= 14) return { date: new Date(number), read_as: "Unix milliseconds" };
    if (digits <= 17) return { date: new Date(number / 1000), read_as: "Unix microseconds" };
    return { date: new Date(number / 1e6), read_as: "Unix nanoseconds" };
  }
  const parsed = Date.parse(text);
  if (Number.isNaN(parsed)) return null;
  return { date: new Date(parsed), read_as: "Date" };
}

export default class ConvertTimestamp extends TextToolView {
  init(props, cx) {
    super.init(props, cx);
    // A selection that is not a time is left alone: start from now instead.
    if (!parseTime(this.selection)) this.selection = "";
    // While showing now, keep it current.
    cx.timer.every(1000, (timer) => {
      if (this.input.trim() === "" || /^now$/i.test(this.input.trim())) timer.notify();
    });
  }

  page() {
    return {
      placeholder: "Unix timestamp, ISO 8601 date, or “now”…",
      empty_title: "Not a timestamp or date",
      empty_description: "Try 1700000000, 1700000000000, 2024-05-01T12:00:00Z or “now”.",
      showing_detail: false,
    };
  }

  rows(input) {
    const time = parseTime(input);
    if (!time || Number.isNaN(time.date.getTime())) return [];
    const { date, read_as } = time;
    const ms = date.getTime();
    const relative = Math.abs(ms - Date.now()) < 1000 ? "now" : relativeTime(ms - Date.now());
    const accessory = read_as;
    return [
      resultItem("seconds", String(Math.floor(ms / 1000)), String(Math.floor(ms / 1000)), {
        icon: "clock",
        subtitle: "Unix seconds",
        accessory,
      }),
      resultItem("milliseconds", String(ms), String(ms), { icon: "timer", subtitle: "Unix milliseconds" }),
      resultItem("iso", date.toISOString(), date.toISOString(), { icon: "globe", subtitle: "ISO 8601, UTC" }),
      resultItem("local-iso", localIso(date), localIso(date), { icon: "map-pin", subtitle: "ISO 8601, local time" }),
      resultItem("local", date.toLocaleString(), date.toLocaleString(), { icon: "calendar", subtitle: "Local time" }),
      resultItem("utc", date.toUTCString(), date.toUTCString(), { icon: "mail", subtitle: "RFC 7231 (HTTP date)" }),
      resultItem("relative", relative, relative, { icon: "hourglass", subtitle: "Relative to now" }),
    ];
  }
}
