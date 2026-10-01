// A form of `bw generate` options; submitting it generates a password or a
// passphrase and copies or pastes it. The password itself is never shown on
// a page. The options (never a password) are remembered in localStorage.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  Checkbox,
  Dropdown,
  DropdownItem,
  Form,
  FormSeparator,
  TextField,
} from "launcher";
import { copy, paste, show_hud, show_toast } from "launcher/api";
import { BwError, generate } from "../lib/bw.js";

const OPTIONS_KEY = "generate-options";
const DEFAULTS = {
  kind: "password",
  length: "20",
  uppercase: true,
  lowercase: true,
  number: true,
  special: true,
  words: "4",
  separator: "-",
  capitalize: true,
  include_number: false,
};

function loadOptions() {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(OPTIONS_KEY) ?? "{}") };
  } catch (_) {
    return { ...DEFAULTS };
  }
}

function clamp(text, low, high, fallback) {
  const value = Number.parseInt(String(text), 10);
  return Number.isFinite(value) ? Math.min(high, Math.max(low, value)) : fallback;
}

export default class GeneratePassword extends View {
  init() {
    this.options = loadOptions();
    this.busy = false;
  }

  changed(id, value, cx) {
    this.options = { ...this.options, [id]: value };
    try {
      localStorage.setItem(OPTIONS_KEY, JSON.stringify(this.options));
    } catch (_) {
      // Remembering the options is a convenience.
    }
    cx.notify();
  }

  run(values, how, cx) {
    if (this.busy) return;
    const options = {
      ...this.options,
      ...values,
      length: clamp(values.length ?? this.options.length, 5, 128, 20),
      words: clamp(values.words ?? this.options.words, 3, 20, 4),
    };
    if (options.kind !== "passphrase" && !options.uppercase && !options.lowercase && !options.number && !options.special) {
      show_toast({ title: "Choose at least one kind of character", style: "failure" });
      return;
    }
    this.busy = true;
    cx.notify();
    cx.spawn(async (task) => {
      try {
        const secret = await generate(options);
        if (how === "paste") {
          paste(secret, { concealed: true });
        } else {
          copy(secret, { concealed: true });
          show_hud(options.kind === "passphrase" ? "Copied Passphrase" : "Copied Password");
        }
      } catch (error) {
        show_toast({
          title: error instanceof BwError ? error.title : "Cannot generate a password",
          message: error instanceof BwError ? error.hint : String(error?.message ?? error),
          style: "failure",
        });
      }
      this.busy = false;
      task.notify();
    });
  }

  render() {
    const options = this.options;
    const bind = (field, id) => field.value(options[id]).on_change((value, cx) => this.changed(id, value, cx));
    const passphrase = options.kind === "passphrase";
    const fields = [
      bind(
        new Dropdown("kind", "Type").children([
          new DropdownItem("password", "Password"),
          new DropdownItem("passphrase", "Passphrase"),
        ]),
        "kind",
      ),
      new FormSeparator(),
    ];
    if (passphrase) {
      fields.push(
        bind(new TextField("words", "Words").placeholder("4").info("3 to 20 words."), "words"),
        bind(new TextField("separator", "Separator").placeholder("-"), "separator"),
        bind(new Checkbox("capitalize", "Capitalize", "Capitalize each word"), "capitalize"),
        bind(new Checkbox("include_number", "Number", "Add a number to one word"), "include_number"),
      );
    } else {
      fields.push(
        bind(new TextField("length", "Length").placeholder("20").info("5 to 128 characters."), "length"),
        bind(new Checkbox("uppercase", "Characters", "Uppercase (A-Z)"), "uppercase"),
        bind(new Checkbox("lowercase", "", "Lowercase (a-z)"), "lowercase"),
        bind(new Checkbox("number", "", "Numbers (0-9)"), "number"),
        bind(new Checkbox("special", "", "Special characters (!@#$%^&*)"), "special"),
      );
    }
    const noun = passphrase ? "Passphrase" : "Password";
    return new Form()
      .loading(this.busy)
      .actions(
        new ActionPanel().children([
          new Action(`Copy ${noun}`).icon("copy").submit((values, cx) => this.run(values, "copy", cx)),
          new Action(`Paste ${noun}`).icon("clipboard-paste").submit((values, cx) => this.run(values, "paste", cx)),
        ]),
      )
      .children(fields);
  }
}
