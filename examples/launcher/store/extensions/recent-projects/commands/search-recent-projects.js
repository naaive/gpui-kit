// One searchable list of the folders and workspaces recently opened in VS
// Code, Cursor, VSCodium, Windsurf and JetBrains IDEs, sectioned by editor,
// newest first, with actions to open one again in the editor it came from.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  List,
  ListDropdown,
  ListDropdownItem,
  ListItem,
  ListSection,
} from "launcher";
import { environment, launch } from "launcher/api";
import { findJetBrainsExecutable, readJetBrains } from "../lib/jetbrains.js";
import { isWindows, platform, tildify } from "../lib/paths.js";
import { CODE_EDITORS, editorUrl, findCodeExecutable, readCodeEditor } from "../lib/vscode.js";

const KIND_ICONS = { folder: "folder", workspace: "layers", file: "file-code" };

/** What `open_with` takes for an editor: a program on Windows, a name elsewhere. */
async function codeApplication(editor) {
  if (isWindows()) return findCodeExecutable(editor);
  return platform() === "macos" ? editor.mac : editor.linux;
}

async function jetBrainsApplication(editor) {
  if (isWindows()) return findJetBrainsExecutable(editor);
  return platform() === "macos" ? editor.title : editor.script;
}

export default class SearchRecentProjects extends View {
  init(_props, cx) {
    this.cx = cx;
    this.groups = [];
    this.filter = "all";
    this.loading = true;
    this.problems = [];
    this.show_files = launch().preferences.show_files === true;
    try {
      this.home = environment().home_path;
    } catch {
      this.home = "";
    }
    // The page shows "Reading…" while a large editor database is copied.
    this.load();
  }

  load() {
    this.cx.spawn(async (task) => {
      const groups = [];
      const problems = [];
      for (const editor of CODE_EDITORS) {
        try {
          const projects = (await readCodeEditor(editor)).filter((project) => this.show_files || project.kind !== "file");
          if (projects.length === 0) continue;
          groups.push({ editor, projects, application: await codeApplication(editor), code: true });
        } catch (error) {
          problems.push(`${editor.title}: ${error?.message ?? error}`);
        }
      }
      try {
        // IDEs used most recently first; VS Code's list has an order but no
        // dates, so those editors keep the order above.
        const ides = (await readJetBrains()).sort((a, b) => (b.projects[0]?.time ?? 0) - (a.projects[0]?.time ?? 0));
        for (const { editor, projects } of ides) {
          if (projects.length === 0) continue;
          groups.push({ editor, projects, application: await jetBrainsApplication(editor), code: false });
        }
      } catch (error) {
        problems.push(`JetBrains: ${error?.message ?? error}`);
      }
      this.groups = groups;
      this.problems = problems;
      this.loading = false;
      task.notify();
    });
  }

  openAction(group, project) {
    const title = `Open in ${group.editor.title}`;
    const action = new Action(title).icon(group.editor.icon);
    if (group.code) {
      // A program takes a local path; a remote folder, or an editor whose
      // program was not found, goes through the editor's own URL scheme.
      if (project.local && group.application) return action.open_with(project.path, group.application);
      return action.open_url(editorUrl(group.editor, project));
    }
    if (group.application) return action.open_with(project.path, group.application);
    // No IDE program found: open the folder itself.
    return null;
  }

  row(group, project) {
    const subtitle = project.remote ? `${project.path} [${project.remote}]` : tildify(project.path, this.home);
    let item = new ListItem(project.id, project.name)
      .icon(group.editor.icon)
      .subtitle(subtitle)
      .keyword(project.path)
      .keyword(group.editor.title);
    if (project.remote) item = item.tag(project.remote, "accent");
    if (project.kind !== "folder") item = item.accessory_icon(KIND_ICONS[project.kind]).accessory_tooltip(project.kind === "file" ? "File" : "Workspace");
    if (project.time) item = item.accessory_date(new Date(project.time).toISOString());
    const more = [];
    if (project.reveal) {
      more.push(new Action("Reveal in File Manager").icon("folder-search").shortcut("secondary-shift-f").reveal(project.reveal));
    }
    more.push(new Action("Copy Path").icon("copy").shortcut("secondary-shift-c").copy(project.path));
    const open = this.openAction(group, project);
    const folder = project.local && project.kind !== "file";
    if (folder && open) {
      more.push(new Action("Open Folder").icon("folder-open").shortcut("secondary-shift-o").open(project.path));
    }
    return item.actions(
      new ActionPanel().children([
        open ?? new Action("Open Folder").icon("folder-open").open(project.path),
        ...more,
        new ActionPanelSection("More").children([
          new Action("Copy Name").icon("copy").copy(project.name),
          new Action("Reload").icon("refresh-cw").shortcut("secondary-r").run((cx) => {
            this.loading = true;
            this.load();
            cx.notify();
          }),
        ]),
      ]),
    );
  }

  render() {
    const shown = this.groups.filter((group) => this.filter === "all" || group.editor.id === this.filter);
    const empty = this.problems.length > 0 && this.groups.length === 0
      ? ["Cannot Read Recent Projects", this.problems.join("\n")]
      : this.groups.length === 0
        ? ["No Recent Projects", "Open a folder in VS Code, Cursor, VSCodium, Windsurf or a JetBrains IDE, and it shows here."]
        : ["No Matching Projects", "Try another name or path."];
    return new List()
      .placeholder("Search recent projects…")
      .loading(this.loading)
      .empty_title(this.loading ? "Reading Recent Projects…" : empty[0])
      .empty_description(this.loading ? "Looking at your editors' history." : empty[1])
      .dropdown(
        new ListDropdown("Editor")
          .value(this.filter)
          .children([
            new ListDropdownItem("all", "All Editors"),
            ...this.groups.map((group) => new ListDropdownItem(group.editor.id, group.editor.title)),
          ])
          .on_change((value, cx) => {
            this.filter = value;
            cx.notify();
          }),
      )
      .children(
        shown.map((group) =>
          new ListSection(group.editor.title)
            .subtitle(String(group.projects.length))
            .children(group.projects.map((project) => this.row(group, project))),
        ),
      );
  }
}
