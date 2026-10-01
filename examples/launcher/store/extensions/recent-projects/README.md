# Recent Projects

Reopen the folders and workspaces you worked on recently, from every editor
in one list.

## Commands

- **Search Recent Projects**: recent projects of Visual Studio Code, VS Code
  Insiders, Cursor, VSCodium, Windsurf and JetBrains IDEs (IntelliJ IDEA,
  PyCharm, WebStorm, CLion, GoLand, Rider, RustRover and others), in a section
  per editor, newest first. Open a project in the editor it came from, reveal
  it in the file manager, copy its path, or open the folder. The dropdown
  shows one editor's projects.

Remote folders of VS Code-based editors (WSL, SSH, containers) are listed
with their remote and reopened through the editor's own URL scheme; a WSL
folder can also be revealed in Explorer through `\wsl.localhost`.

## Where it reads

- VS Code-based editors: `history.recentlyOpenedPathsList` in
  `<config>/<Editor>/User/globalStorage/state.vscdb`, read-only, from a copy.
- JetBrains IDEs: `<config>/JetBrains/<Product><version>/options/recentProjects.xml`.

`<config>` is `%APPDATA%` on Windows, `~/Library/Application Support` on
macOS and `~/.config` on Linux.

To open a project, the extension finds the editor's program on Windows in
its usual install folders (`%LOCALAPPDATA%\Programs`, `C:\Program Files`, or
JetBrains Toolbox's scripts); when it is not found, a VS Code-based editor is
opened through its URL scheme (`vscode://file/…`) and a JetBrains project
opens as a folder. On macOS the editor is opened by its application name,
and on Linux by its command (`code`, `cursor`, `codium`, `windsurf`, or a
Toolbox script such as `idea`).

## Setup

None. **Recent Files** in the preferences also lists single files opened in
VS Code-based editors.

## Permissions

Read-only file access to the editors' state folders listed above, and, to find
the editors' programs on Windows, to `%LOCALAPPDATA%\Programs`,
`%LOCALAPPDATA%\JetBrains\Toolbox\scripts` and the editors' folders under
`C:\Program Files`. No network.
