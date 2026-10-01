// Lists local Docker images with their size and age, with actions to run a
// container from one, copy its name or ID, or remove it.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  Checkbox,
  Detail,
  Form,
  FormDescription,
  List,
  ListItem,
  MetadataLabel,
  TextField,
} from "launcher";
import { pop, show_toast } from "launcher/api";
import { describe, docker, invoke, parseLines } from "../lib/docker.js";

function toImage(raw) {
  const repository = raw.Repository ?? "<none>";
  const tag = raw.Tag ?? "<none>";
  const dangling = repository === "<none>";
  const id = String(raw.ID ?? "").replace(/^sha256:/, "");
  return {
    key: `${id}:${repository}:${tag}`,
    id,
    short: id.slice(0, 12),
    repository,
    tag,
    dangling,
    // What `docker run` and `docker rmi` take: the name when there is one.
    reference: dangling ? id : tag === "<none>" ? repository : `${repository}:${tag}`,
    title: dangling ? `<none> (${id.slice(0, 12)})` : `${repository}:${tag}`,
    size: raw.Size ?? raw.VirtualSize ?? "Unknown",
    created: raw.CreatedSince ?? "",
    createdAt: raw.CreatedAt ?? "",
  };
}

function detail(image) {
  const escaped = image.title.replace(/[_<>]/g, (character) => `\\${character}`);
  return new Detail(`# ${escaped}\n\n\`\`\`sh\ndocker run --rm -it ${image.reference}\n\`\`\``).children([
    new MetadataLabel("Repository", image.repository),
    new MetadataLabel("Tag", image.tag),
    new MetadataLabel("ID", image.short),
    new MetadataLabel("Size", image.size),
    new MetadataLabel("Created", image.created || "Unknown"),
  ]);
}

/** `8080:80, 5432:5432` → `["-p", "8080:80", "-p", "5432:5432"]`. */
function portArguments(text) {
  return String(text ?? "")
    .split(/[\s,]+/)
    .filter(Boolean)
    .flatMap((mapping) => ["--publish", mapping]);
}

class RunImage extends View {
  init({ image, on_started }) {
    this.image = image;
    this.on_started = on_started;
    this.errors = {};
  }

  validate(name, ports) {
    const errors = {};
    if (name && !/^[a-zA-Z0-9][a-zA-Z0-9_.-]*$/.test(name)) {
      errors.name = "Use letters, digits, _, . and -, starting with a letter or digit";
    }
    const mappings = ports.split(/[\s,]+/).filter(Boolean);
    if (!mappings.every((mapping) => /^[0-9a-fA-F.:[\]-]+(\/(tcp|udp))?$/.test(mapping))) {
      errors.ports = "Write host:container pairs, such as 8080:80";
    }
    return errors;
  }

  submit(values, cx) {
    const name = String(values.name ?? "").trim();
    const ports = String(values.ports ?? "").trim();
    this.errors = this.validate(name, ports);
    cx.notify();
    if (Object.keys(this.errors).length > 0) return;
    const args = ["run", "--detach"];
    if (values.remove) args.push("--rm");
    if (name) args.push("--name", name);
    args.push(...portArguments(ports), this.image.reference);
    cx.spawn(async () => {
      show_toast({ id: "docker-run", title: `Starting ${this.image.title}…`, style: "progress" });
      try {
        const { stdout } = await invoke(args);
        show_toast({ id: "docker-run", title: `Started ${name || stdout.trim().slice(0, 12)}`, style: "success" });
        this.on_started?.();
        pop();
      } catch (error) {
        const [title, message] = describe(error);
        show_toast({ id: "docker-run", title, message, style: "failure" });
      }
    });
  }

  render() {
    const name = new TextField("name", "Container Name").placeholder("Optional");
    const ports = new TextField("ports", "Published Ports")
      .placeholder("8080:80, 5432:5432")
      .info("host:container pairs, separated by commas");
    return new Form()
      .actions(new ActionPanel().child(new Action("Run Container").icon("play").submit((values, cx) => this.submit(values, cx))))
      .children([
        new FormDescription("Image", this.image.title),
        this.errors.name ? name.error(this.errors.name) : name,
        this.errors.ports ? ports.error(this.errors.ports) : ports,
        new Checkbox("remove", "Cleanup", "Remove the container when it stops").default_value(true),
      ]);
  }
}

export default class SearchImages extends View {
  init(_props, cx) {
    this.cx = cx;
    this.images = [];
    this.loading = true;
    this.error = null;
    this.reload();
  }

  reload() {
    this.loading = true;
    this.cx.spawn(async (task) => {
      try {
        const output = await docker(["images", "--format", "{{json .}}"]);
        this.images = parseLines(output).map(toImage);
        this.error = null;
      } catch (error) {
        this.images = [];
        this.error = describe(error);
      } finally {
        this.loading = false;
        task.notify();
      }
    });
  }

  remove(image) {
    this.cx.spawn(async (task) => {
      show_toast({ id: "docker-rmi", title: `Removing ${image.title}…`, style: "progress" });
      try {
        await invoke(["rmi", image.reference]);
        show_toast({ id: "docker-rmi", title: `Removed ${image.title}`, style: "success" });
      } catch (error) {
        const [title, message] = describe(error);
        show_toast({ id: "docker-rmi", title, message, style: "failure" });
      }
      this.reload();
      task.notify();
    });
  }

  row(image) {
    const item = new ListItem(image.key, image.title)
      .icon("layers")
      .keyword(image.short)
      .accessory(image.size)
      .accessory_tooltip("Size");
    return (image.created ? item.accessory(image.created) : item)
      .detail(detail(image))
      .actions(
        new ActionPanel().children([
          new Action("Run Container…")
            .icon("play")
            .push(() => new RunImage({ image, on_started: () => this.reload() }), `Run ${image.title}`),
          new Action("Copy Image Name").icon("copy").shortcut("secondary-shift-c").copy(image.reference),
          new Action("Copy Image ID").icon("copy").copy(image.id),
          new Action("Copy Run Command").icon("terminal").copy(`docker run --rm -it ${image.reference}`),
          new ActionPanelSection("Manage").children([
            new Action("Refresh").icon("refresh-cw").shortcut("secondary-r").run((cx) => {
              this.reload();
              cx.notify();
            }),
            new Action("Remove Image")
              .icon("trash")
              .shortcut("secondary-shift-x")
              .destructive()
              .confirm(`Remove ${image.title}?`, "Docker refuses while a container still uses the image.")
              .run(() => this.remove(image)),
          ]),
        ]),
      );
  }

  render() {
    const [title, description] = this.error
      ? this.error
      : this.images.length === 0
        ? ["No Images", "Images you pull or build show here."]
        : ["No Matching Images", "Try another repository, tag or ID."];
    return new List()
      .placeholder("Search images by repository, tag or ID…")
      .loading(this.loading)
      .showing_detail(this.images.length > 0)
      .empty_title(this.loading && this.images.length === 0 ? "Asking Docker…" : title)
      .empty_description(this.loading && this.images.length === 0 ? "Listing images." : description)
      .children(this.images.map((image) => this.row(image)));
  }
}
