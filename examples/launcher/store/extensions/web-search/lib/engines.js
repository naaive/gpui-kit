// The search engines: where their results pages are, and how to read their
// public suggestion endpoints. Each `suggest` answers `[{ title, url? }]`;
// a `url` opens that page directly (a Wikipedia article) instead of searching.

async function getJson(url) {
  const response = await fetch(url, { headers: { Accept: "application/json" } });
  if (!response.ok) throw new Error(`The suggestion service answered ${response.status}.`);
  return JSON.parse(await response.text());
}

/** The OpenSearch shape most engines answer: `[query, [suggestions], …]`. */
function openSearch(body) {
  return Array.isArray(body) && Array.isArray(body[1]) ? body[1].filter((each) => typeof each === "string") : [];
}

function google(dataset) {
  return async (query) => {
    const extra = dataset ? `&ds=${dataset}` : "";
    const body = await getJson(
      `https://suggestqueries.google.com/complete/search?client=firefox&ie=utf-8&oe=utf-8${extra}&q=${encodeURIComponent(query)}`,
    );
    return openSearch(body).map((title) => ({ title }));
  };
}

export const ENGINES = [
  {
    id: "google",
    title: "Google",
    icon: "search",
    search_url: (query) => `https://www.google.com/search?q=${encodeURIComponent(query)}`,
    suggest: google(""),
  },
  {
    id: "bing",
    title: "Bing",
    icon: "search",
    search_url: (query) => `https://www.bing.com/search?q=${encodeURIComponent(query)}`,
    suggest: async (query) =>
      openSearch(await getJson(`https://api.bing.com/osjson.aspx?query=${encodeURIComponent(query)}`)).map((title) => ({
        title,
      })),
  },
  {
    id: "duckduckgo",
    title: "DuckDuckGo",
    icon: "search",
    search_url: (query) => `https://duckduckgo.com/?q=${encodeURIComponent(query)}`,
    suggest: async (query) => {
      const body = await getJson(`https://duckduckgo.com/ac/?q=${encodeURIComponent(query)}`);
      return (Array.isArray(body) ? body : [])
        .map((each) => each?.phrase)
        .filter((phrase) => typeof phrase === "string")
        .map((title) => ({ title }));
    },
  },
  {
    id: "wikipedia",
    title: "Wikipedia",
    icon: "book-open",
    search_url: (query) => `https://en.wikipedia.org/w/index.php?search=${encodeURIComponent(query)}`,
    suggest: async (query) => {
      const body = await getJson(
        `https://en.wikipedia.org/w/api.php?action=opensearch&format=json&limit=10&search=${encodeURIComponent(query)}`,
      );
      const urls = Array.isArray(body?.[3]) ? body[3] : [];
      return openSearch(body).map((title, index) => ({ title, url: urls[index] }));
    },
  },
  {
    id: "youtube",
    title: "YouTube",
    icon: "circle-play",
    search_url: (query) => `https://www.youtube.com/results?search_query=${encodeURIComponent(query)}`,
    suggest: google("yt"),
  },
];

export function engineById(id) {
  return ENGINES.find((engine) => engine.id === id) ?? ENGINES[0];
}
