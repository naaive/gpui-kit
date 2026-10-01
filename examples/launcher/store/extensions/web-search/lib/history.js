// What Web Search remembers in its `localStorage`: the engine used last, and
// the recent searches shown before anything is typed.
const ENGINE_KEY = "last-engine";
const RECENT_KEY = "recent-searches";
const MAX_RECENT = 15;

function read(key, fallback) {
  try {
    const value = localStorage.getItem(key);
    return value === null || value === undefined ? fallback : JSON.parse(value);
  } catch (_) {
    return fallback;
  }
}

function write(key, value) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch (_) {
    // Forgetting is harmless; searching still works.
  }
}

export function lastEngine() {
  const id = read(ENGINE_KEY, null);
  return typeof id === "string" ? id : null;
}

export function rememberEngine(id) {
  write(ENGINE_KEY, id);
}

/** Recent searches, newest first: `[{ query, engine, time }]`. */
export function recentSearches() {
  const list = read(RECENT_KEY, []);
  return Array.isArray(list) ? list.filter((each) => typeof each?.query === "string") : [];
}

export function rememberSearch(query, engine) {
  const others = recentSearches().filter((each) => each.query !== query || each.engine !== engine);
  write(RECENT_KEY, [{ query, engine, time: Date.now() }, ...others].slice(0, MAX_RECENT));
}

export function forgetSearch(query, engine) {
  write(
    RECENT_KEY,
    recentSearches().filter((each) => each.query !== query || each.engine !== engine),
  );
}

export function forgetAllSearches() {
  write(RECENT_KEY, []);
}
