# Launcher

A keyboard-first launcher in the style of Raycast. Built-in commands are written
in Rust; extension commands are written in JavaScript and run on GPUI Shell.
Every page, built in or not, is drawn by the launcher itself, so an extension
looks and behaves exactly like a built-in command.

```bash
cargo run -p launcher
```

Type to search, `↑`/`↓` to select, `Enter` for the primary action,
`Cmd/Ctrl-Enter` for the secondary one, `Cmd/Ctrl-K` for every action, `Esc` to
clear the search, go back, and finally hide the window.

The architecture is described in [`docs/LAUNCHER-DESIGN.md`](../../docs/LAUNCHER-DESIGN.md).

## Bundled extensions

Each one is a reference for part of the SDK.

| Extension                         | Shows                                                                                     |
| --------------------------------- | ----------------------------------------------------------------------------------------- |
| [`gpui-kit`](extensions/gpui-kit) | Static lists, action panels, a pushed `Detail`, no-view commands, an argument, a fallback |
| [`notes`](extensions/notes)       | `localStorage`, a detail pane, a `Form` with validation, a confirmed destructive action   |
| [`emoji`](extensions/emoji)       | A grid in sections, a `ListDropdown`, an extension preference that reorders actions       |
| [`github`](extensions/github)     | `on_query_change`, `fetch` under a narrow network grant, a password preference, timers    |

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
| `commands[].mode`                       | `view` (the default) pushes the page `render` returns; `no-view` runs `init` and shows no page                                         |
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
| `icon(name)`                              | A Lucide icon name such as `globe`, or an image path inside the extension       |
| `accessory(text)`, `accessory_icon(name)` | Trailing text or icon                                                           |
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
| `DatePicker`                             | An ISO 8601 `YYYY-MM-DD` string, or `null`                     |

Every field has `value(v)` (the page keeps the value), `default_value(v)` (the
launcher keeps it), `info(text)`, `error(text)` and `on_change((v, cx) => …)`.

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

and may add `icon(name)`, `shortcut(keys)` (such as `secondary-shift-c`, where
`secondary` is Cmd on macOS and Ctrl elsewhere), `destructive()`, and
`confirm(title, message?)` to ask first. Every effect but `run` and `submit` is
carried out by the launcher without running extension code.

### `launcher/api`

| Function                                                                        | Meaning                                                                                                                      |
| ------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `launch()`                                                                      | `{ extension, command, arguments, preferences, launch_type }` of this launch                                                 |
| `show_toast({ title, message?, style?, id? })`                                  | A message in the launcher; `style` is `info`, `success`, `failure` or `progress`, and a toast replaces the one with its `id` |
| `show_hud(text)`                                                                | Hides the launcher and shows a short message                                                                                 |
| `close_main_window()`, `pop()`, `pop_to_root()`                                 | Hides the launcher, or navigates back                                                                                        |
| `open(target)`                                                                  | Opens a URL, file or folder                                                                                                  |
| `copy(text)`, `paste(text)`                                                     | Copies, or pastes into the frontmost application                                                                             |
| `launch_command(name, arguments?)`                                              | Opens another command, as `Action.launch` does                                                                               |
| `environment()`                                                                 | `{ appearance, locale, launcher_version, development }`                                                                      |
| `cache_get(key)`, `cache_set(key, value)`, `cache_remove(key)`, `cache_clear()` | This extension's JSON cache, up to 10 MB                                                                                     |
| `update_command_metadata({ subtitle })`                                         | Changes how the root search shows this command; `null` restores the manifest's                                               |

Keep data that must survive in `localStorage`; keep what can be fetched again
in the cache.

### `launcher/utils`

Helpers a view keeps in a field. Each takes the `cx` that `init(props, cx)`
receives and calls `cx.notify()` itself when its state changes.

| Export                                                                | Meaning                                                                                                                                                                                                |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `new Query(cx, loader, options?)`                                     | Keeps `data`, `loading` and `error` of an async `loader`; `revalidate()`, `mutate(data)`. Options: `cache_key` to show the last result at once, `initial`, `execute`, `failure_title`, `failure_toast` |
| `new Paginator(cx, loader, options?)`                                 | Appends pages from `loader(page, cursor)`, which answers `{ items, has_more, cursor? }`, for `List.on_load_more`; `load_more()`, `reset()`                                                             |
| `new FormState(initial, rules)`                                       | A form's values and errors: `bind(id, field)`, `validate(values)`, `value(id)`, `error(id)`, `set`, `reset`; `FormState.required(message)` is a rule                                                   |
| `new Frecency(cache_key?)`, `frecency_sort(items, key, visits, now?)` | Orders items by how often and how recently they were used; `visit(id)`, `sort(items, key)`                                                                                                             |
