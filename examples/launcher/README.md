# Launcher

A keyboard-first launcher in the style of Raycast. Built-in commands are written
in Rust; extension commands are written in JavaScript and run on GPUI Shell.
Every page, built in or not, is drawn by the launcher itself, so an extension
looks and behaves exactly like a built-in command.

```bash
cargo run -p launcher
```

The architecture is described in [`docs/LAUNCHER-DESIGN.md`](../../docs/LAUNCHER-DESIGN.md).

## Using the launcher

| Key                          | Does                                                                   |
| ---------------------------- | ---------------------------------------------------------------------- |
| typing                       | Searches the current page                                              |
| `↑` `↓`, `Ctrl-P` `Ctrl-N`   | Moves the selection; in a grid, `←` `→` move within a row              |
| `Enter`                      | Performs the selected item's first action                              |
| `Cmd/Ctrl-Enter`             | Performs its second action; on a form, submits it                      |
| `Cmd/Ctrl-K`                 | Shows every action of the selection or page, searchable                |
| `Tab`, `Shift-Tab`           | Moves between form fields, and to a list's filter beside the search    |
| `Esc`                        | Closes the action panel, clears the search, goes back, then hides      |

The root search lists applications, extension commands, quicklinks, snippets,
script commands and the launcher's own commands (appearance, settings, Manage
Extensions, system commands such as lock and sleep where the platform has them).
It ranks what you pick more often higher, remembers what you picked for a
query, matches Chinese names by pinyin and initials (`wx` finds 微信), answers
arithmetic (`2^10`), units (`5 km to mi`, `100 f in c`, `1.5 GB in MiB`),
percentages (`20% of 150`, `80 - 15%`), number bases (`255 in hex`), currencies
(`100 usd to twd`, `€25 in yen`; daily rates from open.er-api.com, cached for a
day), time zones (`time in tokyo`, `5pm pst to taipei`) and dates (`days until
christmas`, `today + 30 days`), shows a typed color (`#f80`, `rgb(255 136 0)`,
`hsl(32 100% 50%)`) in every notation, and ends with "Use “…” with…" for a web search,
every fallback command and every quicklink that takes one argument.

Every root item can be customized from its actions (`Cmd/Ctrl-K`): **Add to
Favorites** (favorites lead the empty search), **Set Alias…** (typing the alias
puts the item first), **Set Hotkey…** (a system-wide shortcut that opens it) and
**Copy Deeplink**.

Quicklinks and snippets fill in `{selection}` with the text selected in the
application that was in front when the launcher opened (read through the
accessibility API, so nothing is typed into it). With **Hyper Key** on in
Settings, holding Caps Lock is Ctrl+Shift+Alt+Win, for hotkeys such as
`hyper-k`; a quick press stays Caps Lock, becomes Esc, or does nothing.

### Built-in commands

| Command                                     | Does                                                                                                                                          |
| ------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| Clipboard History                           | Text, links, colors, images and files you copied, by day, with a preview; paste or copy again, pin, delete. Private copies are never recorded, nor copies from applications you exclude (password managers by default); text in copied images is recognized and searchable (Windows) |
| Search Files                                | Files and folders in your home folder by name, recent files with nothing typed, a preview of text, images, a folder's contents and the system's thumbnail of documents and videos; Quick Look shows it larger |
| Create Quicklink, Search Quicklinks         | Saved links and paths, opened from the search; `{argument}`, `{clipboard}`, `{date}` and `{time}` are filled in when one opens                |
| Create Snippet, Search Snippets             | Saved text to paste, with the same placeholders; a keyword puts it first, and can expand as you type anywhere (Windows, opt-in in Settings, except in applications you list)   |
| Search Emoji & Symbols                      | Every emoji by name and shortcode, with a skin tone chosen once, and arrows, math, currency, keyboard and box-drawing symbols; recently used first |
| Switch Windows                              | Every open window, front to back, to switch to, minimize or close (Windows)                                                                    |
| Pick Color, Search Colors                   | A magnifier beside the pointer; a click copies the color under it as HEX (Esc cancels, arrows nudge). Picked colors are kept to copy as HEX, RGB, HSL (Windows) |
| Toggle Floating Notes, Create Note, Search Notes | Markdown notes in a small window above the others, saved as you type; searched and previewed from the launcher                         |
| My Schedule                                 | Events of the calendars you subscribe to by iCal address (Google Calendar, Outlook) for the next two weeks; a meeting about to start is offered in the root search to join |
| Start Focus Session                         | A goal and a length; social media, video, news, games, chat or your own apps and sites are minimized while it runs, websites read from the browser's address bar (Windows) |
| Search Screenshots                          | Screenshots in the Screenshots folder, newest first, found by the text in them (Windows)                                                       |
| Change Theme                                | Orbit, the launcher's own (the default), GPUI Kit's themes, and your own from the `themes` folder, one for light and one for dark appearance |
| Export Settings & Data, Import…             | Everything but the clipboard history in one JSON file, imported elsewhere without a restart                                                    |
| Create Script Command                       | Scripts in the script commands folder become commands; Raycast's `@raycast.title`, `mode`, `icon` and `argument1…3` comments are understood   |
| Search Processes                            | Running programs with CPU and memory, to quit or force quit                                                                                   |
| Search Bookmarks                            | Bookmarks of Chrome, Edge, Brave, Vivaldi and Chromium                                                                                        |
| Search Browser History                      | Pages visited in Chrome, Edge, Brave, Vivaldi, Chromium and Firefox, newest first, by title or address                                       |
| Search Browser Tabs                         | The tabs open in every browser window, to switch to (Windows)                                                                                 |
| Search Menu Items                           | The menu commands of the window that was in front, to run by name; a drawn menu bar's menus are opened (Windows)                             |
| Search Reminders, Create Reminder           | Reminders with a due time typed as `tomorrow 9am`, `in 2 hours` or `明天下午3点`, and a priority; a small alert opens in the corner when one comes due, and due ones show in the root search |
| Calculator History                          | Calculations and conversions whose answer you copied or pasted                                                                                |
| Translate                                   | The typed or selected text in another language, through Google Translate                                                                      |
| Define Word                                 | An English or Chinese word's meanings, pronunciation and examples, from Wiktionary                                                            |
| System Monitor                              | CPU, memory, disks, network, battery and uptime, refreshed every two seconds                                                                  |
| Now Playing, Play/Pause Media, Next Track, Previous Track | What each app is playing, with artwork and position, and its controls (Windows) |
| Toggle Wi-Fi, Toggle Bluetooth, Network Status | Turn the radios on or off, and see the Wi-Fi network and signal (Windows) |
| Keep Awake, Allow Sleep | Keep the computer (and optionally the display) awake until stopped, for a while or until a time; the subtitle says until when |
| Start Timer, Running Timers, Stopwatch | Timers typed as `25m tea`, `1h 30m` or `10分钟`, with pause, restart and +5 minutes, an alert when one ends, and a stopwatch with laps |
| Eject Drives, Eject All Drives | Removable and external drives with their space, ejected safely and told when one is in use |
| Extension Store | Extensions to install from the store, with their README and screenshots, and updates when there are any |
| Left Half, Maximize, Center, Next Display…  | Window Management for the window that was in front: halves (pressed again: two thirds, then one third), thirds, fourths, sixths, quarters, moving to an edge, larger and smaller, other displays, restore, and layouts of your own; a gap between windows in Settings (Windows) |
| Display Settings, Sound Settings…           | Pages of the system settings (Windows)                                                                                                        |

### From the command line

```text
launcher                 start, or show the launcher already running
launcher toggle          show, or hide it if it is in front
launcher show | hide
launcher open <url>      open a deep link: launcher://extensions/<id>/<command>?arguments=<JSON>
                         (and &context=<JSON>); built-in commands are
                         launcher://extensions/launcher/<name>
launcher dev <dir>       load an extension directory ahead of the installed ones, and
                         reload it whenever one of its files is saved
launcher types <dir>     write TypeScript declarations and the launcher.json schema
launcher new <dir> [--template list|detail|form|no-view|menu-bar]
                         start an extension from a template
launcher lint <dir>      check an extension's manifests, modules and images without
                         running it
launcher store-index <dir>
                         write the index.json of an Extension Store folder
```

Set `LAUNCHER_LOG=info` (any `tracing` filter) to print what the launcher and
extensions report, `console.log` included, to stderr.

Only one launcher runs; the others hand their request to it over a local
socket (a named pipe on Windows). The summon shortcut defaults to `Alt-Space`
and is changed in Launcher Settings. Wayland has no global shortcuts for
applications: bind `launcher toggle` in your desktop's keyboard settings.

### Settings

Launcher Settings opens a window of its own, as Raycast's does, and every
change applies at once:

- **General**: the launcher's hotkey (click the field and press the keys),
  starting at login (`launcher start --background`), the tray icon, the
  appearance and a theme for each, text size, and the window mode.
- **Extensions**: every command, the launcher's own grouped by feature and
  then each extension's, in one searchable table with its type, alias,
  hotkey and whether it is enabled. A disabled command leaves the root search
  and its hotkey does nothing. Beside the table: the selected command's
  alias, hotkey (recorded by pressing it; one already taken is refused) and
  the settings of the feature or extension, such as how long Clipboard
  History keeps entries, the folders Search Files looks in, the calculator's
  decimal separator, or an extension's token.
- **Advanced**: the screen the launcher opens on, when it returns to the root
  search, the keys that move the selection (Ctrl-N/P or Ctrl-J/K), what Esc
  does, the Hyper Key, how loosely the search matches, the extensions folder
  and store, a proxy, and exporting or importing your data.

`launcher open launcher://settings` opens it, and
`launcher://settings/<extension-id>`, `…/<command-id>` or `…/advanced` at that
place; Configure Extension in a command's actions does the same.

### Where things are kept

Under the platform data directory (for example
`~/.local/share/gpui-kit-launcher` on Linux, `~/Library/Application
Support/gpui-kit-launcher` on macOS): `settings.json`, `usage.json` (ranking),
`permissions.json`, `preferences.json`, `quicklinks.json`, `snippets.json`,
`customizations.json` (aliases, favorites, hotkeys, disabled commands), `currency-rates.json`,
`colors.json`, `emoji.json`, `focus.json`, `reminders.json`,
`calculator-history.json`, `store-installs.json` (what came from the
Extension Store), `background-commands.json` (the menu-bar and
interval commands you turned on), `window-layouts.json`, `notes/`
(one Markdown file per note), `calendars/` (cached feeds),
`screenshot-text.json`, `themes/` (your own themes),
`clipboard/` (the clipboard
history and copied images), `script-commands/`, and `extensions/` for extensions
installed from Git. Password preferences go to the system keychain; where none
is available they fall back to an owner-only `secrets.json`, which is not
encrypted.

### Installing extensions

Manage Extensions installs an extension from a Git URL or `owner/repository`,
updates it, reveals it, opens its preferences, or removes it with its
permissions, preferences, secrets and data. Before an extension's code first
runs, the launcher lists what its `gpui-shell.json` asks for (network hosts,
folders, commands it may run, clipboard) and runs it only with what you
allow; an update that asks for more asks again. Required preferences and
arguments are asked for in a form before the command opens.

### Extension Store

The Extension Store command lists the extensions of [`store/`](store), read
from GitHub: each with its README, commands, version and categories, to
install, update when its version changes, or uninstall. Installing downloads
exactly the files `store/index.json` names and checks each one's SHA-256
before the extension replaces anything; installed extensions sit beside those
installed from Git and ask for their permissions the same way. Images in an
extension's `metadata/` folder are shown as screenshots and not installed.

A few seconds after it starts, and every six hours, the launcher asks the
store whether what came from it has a newer version, and shows how many as
the Extension Store command's subtitle; the store page lists them with Update
All. Nothing is updated without you.

The listing comes from `naaive/gpui-kit@main/examples/launcher/store` unless
Launcher Settings (or `LAUNCHER_STORE`) names another: `owner/repo@branch/folder`
on GitHub, or a folder on this computer. Publishing an extension is committing
its folder under `store/extensions/` and running `launcher store-index store`.

## Bundled extensions

Each one is a reference for part of the SDK.

| Extension                         | Shows                                                                                     |
| --------------------------------- | ----------------------------------------------------------------------------------------- |
| [`gpui-kit`](extensions/gpui-kit) | Static lists, action panels, a pushed `Detail`, no-view commands, an argument, a fallback, a `menu-bar` command, an `interval` |
| [`github`](extensions/github)     | `on_query_change`, `fetch` under narrow per-method `network.http` grants (REST and GraphQL), a password preference, sectioned lists, a token empty state, a `menu-bar` command with an `interval` |

## Writing an extension

An extension is a directory with two manifests and a module per command:

```text
hello/
├── gpui-shell.json     what the code may do
├── launcher.json       which commands exist
└── commands/
    └── hello.js
```

Run the launcher with `LAUNCHER_EXTENSIONS=<directory containing hello>` to load
it ahead of the bundled ones; an extension there shadows a bundled one with the
same id.

### `gpui-shell.json`

Identifies the extension and declares its capabilities. It grants nothing by
default except the extension's own `localStorage`:

```json
{
  "id": "com.example.hello",
  "name": "Hello",
  "version": "0.1.0",
  "entry": "commands/hello.js",
  "capabilities": {
    "network": {
      "http": [
        {
          "host": "api.example.com",
          "methods": ["GET"],
          "paths": ["/v1/search"]
        }
      ]
    }
  }
}
```

`entry` is required by GPUI Shell; the launcher loads each command's own
`module` instead. Actions that only describe an effect (opening a URL, copying,
pasting) need no capability, because the launcher performs them. See GPUI
Shell's manifest schema for `fs`, `clipboard` and `process` grants.

### `launcher.json`

Lists the commands. The launcher reads it without running any code, so every
command is searchable at once, and rejects a manifest that breaks a rule below
with the path of the offending field.

```json
{
  "$schema": "./launcher.schema.json",
  "icon": "hand",
  "commands": [
    {
      "name": "hello",
      "title": "Say Hello",
      "subtitle": "Greets someone",
      "keywords": ["greet"],
      "module": "commands/hello.js",
      "arguments": [{ "name": "who", "placeholder": "Name" }]
    }
  ],
  "preferences": [
    {
      "name": "greeting",
      "title": "Greeting",
      "type": "dropdown",
      "default": "hello",
      "choices": [
        { "value": "hello", "title": "Hello" },
        { "value": "hi", "title": "Hi" }
      ]
    }
  ]
}
```

| Field                                   | Meaning                                                                                                                                |
| --------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `icon`                                  | A Lucide icon name, or an image path inside the extension; the default for every command                                               |
| `commands[].name`                       | Lowercase letters, digits and `-`, unique in the extension; `launch().command`                                                         |
| `commands[].title`                      | What the root search shows and matches; `subtitle`, `icon` and `keywords` are optional                                                 |
| `commands[].module`                     | The module default-exporting the command's `View`, relative to the extension                                                           |
| `commands[].mode`                       | `view` (the default) pushes the page `render` returns; `no-view` runs `init` and shows no page; `menu-bar` shows the `MenuBarExtra` `render` returns in the system tray |
| `commands[].interval`                   | How often the launcher runs a `no-view` or `menu-bar` command on its own, such as `10m` (`s`, `m`, `h`, `d`; at least `1m`, or `10s` for `menu-bar`); none of its arguments may be required |
| `commands[].arguments`                  | Up to three inputs typed in the search field before the command runs: `name`, `placeholder`, `required`, `type` (`text` or `password`) |
| `commands[].fallback`                   | Offered when nothing matches, with the search text as its first argument, which must be `text`                                         |
| `preferences`, `commands[].preferences` | Settings filled in once, for the extension or one command; names are shared by both lists                                              |

A preference has a `name` (a letter, then letters, digits or `_`), a `title`, an
optional `description`, `required` and `default`, and a `type`:

- `text` and `password` take a string `default`; a password is kept in the
  system keychain.
- `checkbox` needs a `label`, the text beside the box, and takes a boolean
  `default`.
- `dropdown` needs `choices` of `{ value, title }`; its `default` must be one of
  the values.

A `menu-bar` command, or one with an `interval`, starts running on its own once
you open it from the root search, and keeps doing so after a restart until you
choose Remove from Tray (in its menu) or Stop Running in Background (in its
root search actions). The launcher then runs it with
`launch().launch_type === "background"`; a question it would have to ask first
(a permission, a preference) waits until you open it again.

### Reading other applications' files

`capabilities.fs.read` and `fs.write` take folders, which may start with
`${pluginDir}` (the extension), `${dataDir}` (its own data folder),
`${homeDir}` (your home folder) or `${configDir}` (where applications keep
their settings: `%APPDATA%`, `~/Library/Application Support` or `~/.config`).
`environment()` answers `home_path`, `config_path` and `platform` so the code
can build the same paths. The permission prompt names the whole home or
settings folder in bold.

A program run with `process.run(command, args, { input, env })` (under an
`fs.execute` grant) gets the variables that say where your folders are (home,
app data, temp, `PATH`) and nothing else of the launcher's environment; `env`
adds variables of the extension's own, such as one a program reads a password
from, and `input` is written to its standard input.

### Packages

`gpui-shell.json` lists JavaScript packages under `dependencies`, each a Git
repository holding ES modules (`"dayjs": "iamkun/dayjs#v1.11.13"`, or an
object with a `url`, a `branch` or `tag`, and an `entry`). GPUI Shell checks
them out once into its cache and resolves `import "dayjs"` to them; an
editor finds them through the links it writes beside the extension.

### A command

A command module default-exports a GPUI Shell `View`. Its `render` returns a
page: a `List`, a `Detail` or a `Form`. `init(props, cx)` runs once, before the
first render; keep state in fields and call `cx.notify()` after changing it.

```js
import { View } from "gpui-kit";
import { Action, ActionPanel, List, ListItem } from "launcher";
import { launch, show_toast } from "launcher/api";

export default class Hello extends View {
  init() {
    const { arguments: args, preferences } = launch();
    this.greeting = `${preferences.greeting ?? "hello"}, ${args.who || "world"}`;
  }

  render() {
    return new List().child(
      new ListItem("greeting", this.greeting)
        .subtitle("Press Enter")
        .actions(
          new ActionPanel().children([
            new Action("Greet").run(() =>
              show_toast({ title: this.greeting, style: "success" }),
            ),
            new Action("Copy Greeting")
              .shortcut("secondary-shift-c")
              .copy(this.greeting),
          ]),
        ),
    );
  }
}
```

Types for an editor come from the launcher: it writes `gpui-kit.d.ts`,
`launcher.d.ts`, `launcher-api.d.ts`, `launcher-utils.d.ts` and
`launcher.schema.json` beside the extension, and a `jsconfig.json` once, which
is then yours.

## SDK reference

Three modules. `launcher` holds the page nodes, `launcher/api` asks the launcher
to do things, and `launcher/utils` holds plain JavaScript helpers for state.

### `launcher`: pages

Every node is built with `new`, configured with chained methods, and given
children with `child(node)` or `children([nodes])`. A method that takes a
boolean may be called without one to mean `true`.

**`List()`**, a searchable list or grid. Children are `ListSection` and `ListItem`.

| Method                                         | Meaning                                                                                   |
| ---------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `placeholder(text)`                            | Placeholder of the search field                                                           |
| `loading(bool?)`                               | Shows that results are on their way                                                       |
| `filtering(bool?)`                             | Whether the launcher filters items by the search text; on unless `on_query_change` is set |
| `empty_title(text)`, `empty_description(text)` | What shows when no item matches                                                           |
| `showing_detail(bool?)`                        | Shows the selected item's `detail` beside the list                                        |
| `grid(columns)`                                | Lays the items out in square cells, 1 to 12 per row                                       |
| `selected_item(id)`                            | Selects an item by id                                                                     |
| `search_text(text)`                            | Puts text in the search field as if typed, once per different text, such as the selected text a command starts from |
| `dropdown(ListDropdown)`                       | A filter beside the search field                                                          |
| `on_query_change((text, cx) => …)`             | Called as the user types; the command searches by itself                                  |
| `on_selection_change((id, cx) => …)`           | Called when another item is selected                                                      |
| `on_load_more((cx) => …)`                      | Called near the end of the list, to append the next page                                  |

**`ListSection(title)`** groups items under a header; `subtitle(text)`.

**`ListItem(id, title)`** is one row. Keep `id` stable across renders so the
selection stays on the item.

| Method                                    | Meaning                                                                         |
| ----------------------------------------- | ------------------------------------------------------------------------------- |
| `subtitle(text)`                          | Secondary text after the title                                                  |
| `icon(name)`                              | A Lucide icon name such as `globe`, an image path inside the extension, or an `https://` image |
| `file_icon(path)`                         | The icon the system shows for a file, folder or application                     |
| `icon_tone(tone)`, `icon_mask("circle")`  | Draws a Lucide icon in a tone, or clips the icon to a circle (an avatar)        |
| `accessory(text)`, `accessory_icon(name)` | Trailing text or icon                                                           |
| `accessory_date(date)`                    | A `YYYY-MM-DD` or ISO 8601 date shown relative to now (`3h ago`, `in 2d`), in full on hover |
| `accessory_tooltip(text)`                 | Explains the accessory, icon or tag added just before, on hover                 |
| `tag(text, tone?)`                        | A trailing tag; `tone` is `neutral`, `accent`, `success`, `warning` or `danger` |
| `keyword(text)`                           | Something the search matches without showing it; call it once per keyword       |
| `detail(Detail)`                          | Shown beside the list while selected and the list is `showing_detail`           |
| `actions(ActionPanel)`                    | The item's actions; replaces any added so far                                   |
| `action(Action)`                          | Appends one action                                                              |

**`ListDropdown(tooltip)`** with `value(value)`, `on_change((value, cx) => …)`
and `ListDropdownItem(value, title)` children.

**`Detail(markdown)`**, one object in full: a Markdown body with a metadata
column. Return it as a page, or pass it to `ListItem.detail`. `loading(bool?)`,
`actions(ActionPanel)`. Children are the metadata:

- `MetadataLabel(label, text)`
- `MetadataLink(label, text, url)`
- `MetadataTags(label)`, with `tag(text, tone?)`
- `MetadataSeparator()`

**`Form()`**, fields to fill in. `loading(bool?)`, `actions(ActionPanel)` whose
first action should `submit`. Children are fields, each built as
`Field(id, title)`:

| Field                                    | Value                                                          |
| ---------------------------------------- | -------------------------------------------------------------- |
| `TextField`, `TextArea`, `PasswordField` | A string; also `placeholder(text)`                             |
| `Checkbox(id, title, label)`             | A boolean                                                      |
| `Dropdown`                               | A string, or `null`; children are `DropdownItem(value, title)` |
| `DatePicker`                             | An ISO 8601 `YYYY-MM-DD` string, or `null`; with `include_time()`, `YYYY-MM-DDTHH:MM` |
| `FilePicker`                             | An array of paths chosen in the system's open panel; `directories()`, `multiple()` |
| `TagPicker`                              | An array of values; children are `TagPickerItem(value, title)` |

Every field has `value(v)` (the page keeps the value), `default_value(v)` (the
launcher keeps it), `info(text)`, `error(text)` and `on_change((v, cx) => …)`.
`FormSeparator()` draws a line between fields, and `FormDescription(text)` or
`FormDescription(label, text)` explains them; neither has a value.

**`MenuBarExtra()`**, what a `menu-bar` command's `render` returns: an icon in
the system tray (the menu bar on macOS) and its menu. `icon(name)` (a Lucide
name, or a PNG or SVG inside the extension), `title(text)` (beside the icon on
macOS, in the tooltip elsewhere), `tooltip(text)`, `loading(bool?)`. Children
are `MenuBarItem(title)` with `subtitle(text)`, `checked(bool?)` and
`action(Action)`; `MenuBarSection(title?)`; `MenuBarSubmenu(title)`; and
`MenuBarSeparator()`. An item's `Action` is performed without the launcher
window where it can be (opening, copying, `run`); one that needs a page shows
the launcher first.

### `launcher`: actions

**`ActionPanel()`** holds `Action`, `ActionPanelSection(title?)` and
`ActionPanelSubmenu(title)` (with `icon` and `shortcut`). The first action is
the primary one (`Enter`), the second the secondary one (`Cmd/Ctrl-Enter`), and
`Cmd/Ctrl-K` shows them all.

**`Action(title)`** takes exactly one effect:

| Effect                                        | Meaning                                                                                               |
| --------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `open_url(url)`                               | Opens a URL in the default browser                                                                    |
| `open(path)`, `reveal(path)`                  | Opens a file or folder with its default application, or shows it in the file manager                  |
| `copy(text)`, `paste(text)`                   | Copies, or pastes into the application that was frontmost and hides the launcher                      |
| `toast(title, style?, message?)`, `hud(text)` | Shows a message; a HUD hides the launcher first                                                       |
| `push(() => new SomeView(props), title?)`     | Pushes the page that View renders; `props` reach its `init`                                           |
| `launch(command)`                             | Opens another command by name, or `extension-id/command`; pass arguments with `argument(name, value)` |
| `pop()`, `pop_to_root()`, `close_window()`    | Navigates back, to the root search, or hides the launcher                                             |
| `run((cx) => …)`                              | Calls back into the extension                                                                         |
| `submit((values, cx) => …)`                   | In a form, calls back with every field's value, keyed by field id                                     |
| `open_with(target, application)`              | Opens a file, folder or URL with an application: its path, or a name such as `notepad` or `Safari`    |
| `trash([paths])`                              | Moves files to the Trash (the Recycle Bin), where the user can put them back                          |
| `quick_look(path)`                            | Shows a file large: an image, text, or the system's preview of a document                             |
| `create_quicklink(name, link)`, `create_snippet(text, name?)` | Opens Create Quicklink or Create Snippet filled in; the user saves it                  |
| `pick_date((date, cx) => …, include_time?)`   | Asks for a date (and time), then calls back with it                                                   |

and may add `icon(name)`, `shortcut(keys)` (such as `secondary-shift-c`, where
`secondary` is Cmd on macOS and Ctrl elsewhere), `destructive()`,
`concealed()` (for `copy` or `paste` of a secret: Clipboard History leaves it
out and the clipboard is cleared after 30 seconds), and
`confirm(title, message?)` to ask first. `open_preferences()` is an effect too:
it opens the extension's and command's preferences, such as for a token. Every effect but `run` and `submit` is
carried out by the launcher without running extension code. A shortcut the
search field already uses (such as `ctrl-x`, Cut, on Linux) never reaches an
action; prefer `secondary-shift-…` combinations.

### `launcher/api`

| Function                                                                        | Meaning                                                                                                                      |
| ------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `launch()`                                                                      | `{ extension, command, arguments, preferences, launch_type, context }` of this launch; `launch_type` is `user_initiated` or `background` |
| `show_toast({ title, message?, style?, id?, primary_action? })`                 | A message in the launcher; `style` is `info`, `success`, `failure` or `progress`, and a toast replaces the one with its `id`. Answers a promise of `"primary"` when its button is pressed, `null` when it goes away |
| `confirm_alert({ title, message?, primary_action?, destructive? })`             | Asks the user; answers a promise of whether they confirmed                                                                   |
| `show_hud(text)`                                                                | Hides the launcher and shows a short message                                                                                 |
| `close_main_window()`, `pop()`, `pop_to_root()`                                 | Hides the launcher, or navigates back                                                                                        |
| `open(target, application?)`                                                    | Opens a URL, file or folder, with its default application or the one named                                                   |
| `selected_text()`                                                               | The text selected in the application in front when the launcher was summoned, or `null`                                      |
| `selected_files()`                                                              | A promise of the files selected in the file manager window in front (Explorer, Finder)                                       |
| `frontmost_application()`, `applications()`                                    | The application that was in front (`{ name, path }`, or `null`), and a promise of every installed one                         |
| `copy(text, { concealed? })`, `paste(text, { concealed? })`                     | Copies, or pastes into the frontmost application; `concealed` keeps a secret out of Clipboard History and clears it after 30 seconds |
| `open_extension_preferences()`                                                  | Opens this extension's and command's preferences                                                                             |
| `launch_command(name, arguments?, context?)`                                    | Opens another command, as `Action.launch` does; it reads any JSON `context` as `launch().context`                            |
| `environment()`                                                                 | `{ appearance, locale, launcher_version, development, assets_path, support_path, home_path, config_path, platform }` |
| `cache_get(key)`, `cache_set(key, value)`, `cache_remove(key)`, `cache_clear()` | This extension's JSON cache, up to 10 MB                                                                                     |
| `update_command_metadata({ subtitle })`                                         | Changes how the root search shows this command; `null` restores the manifest's                                               |
| `oauth_authorize({ provider, authorize_url, token_url, client_id, scope?, extra_parameters? })` | Signs in with OAuth 2.0 (authorization code with PKCE) in the browser and keeps the tokens in the system keychain; answers a promise of `{ access_token, refresh_token?, expires_at?, is_expired, … }`. `token_url` must be allowed for POST by the extension's network grant |
| `oauth_tokens(provider)`, `oauth_refresh(client)`, `oauth_remove_tokens(provider)` | The kept tokens or `null`; new tokens from the refresh token; signing out. `redirect_port` fixes the loopback port for a provider that matches the redirect exactly |
| `sql_query(path, sql, parameters?)`                                             | A promise of the rows of a read-only query on an SQLite database inside an `fs.read` grant, such as an editor's recent projects; read from a copy, so the application holding it is not disturbed |

Keep data that must survive in `localStorage`; keep what can be fetched again
in the cache. The clipboard is GPUI Shell's: `cx.read_from_clipboard()` (with a
`clipboard.read` grant) and `cx.write_to_clipboard(text)`; files are its `fs`
module and programs its `process` module, each under the grant
`gpui-shell.json` asks for.

### `launcher/utils`

Helpers a view keeps in a field. Each takes the `cx` that `init(props, cx)`
receives and calls `cx.notify()` itself when its state changes.

| Export                                                                | Meaning                                                                                                                                                                                                |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `new Query(cx, loader, options?)`                                     | Keeps `data`, `loading` and `error` of an async `loader`; `revalidate()`, `mutate(data)`. Options: `cache_key` to show the last result at once, `initial`, `execute`, `failure_title`, `failure_toast` |
| `new Paginator(cx, loader, options?)`                                 | Appends pages from `loader(page, cursor)`, which answers `{ items, has_more, cursor? }`, for `List.on_load_more`; `load_more()`, `reset()`                                                             |
| `new FormState(initial, rules)`                                       | A form's values and errors: `bind(id, field)`, `validate(values)`, `value(id)`, `error(id)`, `set`, `reset`; `FormState.required(message)` is a rule                                                   |
| `new Frecency(cache_key?)`, `frecency_sort(items, key, visits, now?)` | Orders items by how often and how recently they were used; `visit(id)`, `sort(items, key)`                                                                                                             |
