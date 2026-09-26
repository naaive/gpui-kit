# Launcher

A keyboard-first launcher in the style of Raycast. Built-in commands are written
in Rust; extension commands are written in JavaScript and run on GPUI Shell.
Every page, built in or not, is drawn by the launcher itself, so an extension
looks and behaves exactly like a built-in command.

```bash
cargo run -p launcher
```

Type to search, `↑`/`↓` to select, `Enter` for the primary action,
`Cmd/Ctrl-Enter` for the secondary one, `Esc` to clear the search, go back, and
finally hide the window.

The architecture is described in [`docs/LAUNCHER-DESIGN.md`](../../docs/LAUNCHER-DESIGN.md).

## Writing an extension

An extension is a directory with two manifests and a module per command. See
[`extensions/gpui-kit`](extensions/gpui-kit) for a complete one.

`gpui-shell.json` identifies the extension and declares what its code may do.
It grants nothing by default:

```json
{ "id": "com.example.hello", "name": "Hello", "entry": "commands/hello.js" }
```

`launcher.json` lists the commands. The launcher reads it without running any
code, so every command is searchable immediately:

```json
{
  "icon": "book-open",
  "commands": [
    { "name": "hello", "title": "Say Hello", "keywords": ["greet"], "module": "commands/hello.js" }
  ]
}
```

A command module default-exports a GPUI Shell `View` whose `render` returns a
`List`:

```js
import { View } from "gpui-kit";
import { Action, List, ListItem } from "launcher";
import { show_toast } from "launcher/api";

export default class Hello extends View {
  render() {
    return new List().child(
      new ListItem("world", "Hello, World")
        .subtitle("Press Enter")
        .action(new Action("Greet").run(() => show_toast("Hello!", "success")))
        .action(new Action("Copy Greeting").shortcut("secondary-shift-c").copy("Hello!")),
    );
  }
}
```

Run the launcher with `LAUNCHER_EXTENSIONS=<directory containing your extension>`
to load it ahead of the bundled ones.

### SDK

| Module         | Exports                                                                                      |
| -------------- | -------------------------------------------------------------------------------------------- |
| `launcher`     | `List`, `ListSection`, `ListItem`, `Action`                                                  |
| `launcher/api` | `launch()` — the command being opened, read it in `init`; `show_toast(message, style?)`      |

- `List`: `placeholder`, `loading`, `filtering`, `empty_title`, `on_query_change`.
  The launcher filters items by the search text unless `on_query_change` is set.
- `ListItem(id, title)`: `subtitle`, `icon` (a Lucide name), `accessory`,
  `keyword`, `action`. Keep the id stable across renders so the selection
  follows the item.
- `Action(title)`: exactly one of `open_url`, `copy`, `toast`, `run`, `pop`,
  `close_window`, plus an optional `shortcut`. The first action of an item is
  its primary action and the second its secondary action. Everything except
  `run` is carried out by the launcher, without running extension code.
