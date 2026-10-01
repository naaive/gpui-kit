# Web Search

Search Google, Bing, DuckDuckGo, Wikipedia or YouTube, with suggestions as you
type.

## Command

- **Search the Web**: the first row is always exactly what you typed, and
  Enter opens that engine's results in your browser. The engine's suggestions
  follow. A Wikipedia suggestion opens its article directly. Pick the engine
  from the dropdown, or search with another one from the action panel
  (Ctrl/Cmd-K). Before you type, your recent searches are listed, and each can
  be removed. The command is also offered as a fallback when the root search
  finds nothing, so you can type a query and search it straight away.

## Preferences

- **Default Search Engine**: the engine a search starts with.
- **Last Engine**: start with the engine you used last instead (on by default).

## Permissions

GET requests to these suggestion endpoints only:

- `suggestqueries.google.com/complete/search` (Google and YouTube)
- `api.bing.com/osjson.aspx`
- `duckduckgo.com/ac/`
- `en.wikipedia.org/w/api.php`

Results pages open in your browser, which needs no permission. Recent searches
and the last engine stay in the extension's own storage.
