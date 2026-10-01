// Generates passwords from `crypto.randomBytes`, without modulo bias. The
// command's preferences choose the default length and which characters to
// use; the dropdown changes the length for this run.
import { View } from "gpui-kit";
import { randomBytes } from "crypto";
import { Action, ActionPanel, List, ListDropdown, ListDropdownItem, ListItem } from "launcher";
import { launch } from "launcher/api";

const LENGTHS = [8, 12, 16, 20, 24, 32, 48, 64];
const COUNT = 6;
const LOWER = "abcdefghijklmnopqrstuvwxyz";
const UPPER = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS = "0123456789";
const SYMBOLS = "!@#$%^&*()-_=+[]{};:,.?/~";
const AMBIGUOUS = /[Il1O0o|`'"]/g;

/** A uniformly random index below `size`, from rejection sampling. */
function randomIndex(size) {
  const limit = 256 - (256 % size);
  for (;;) {
    const [byte] = randomBytes(1);
    if (byte < limit) return byte % size;
  }
}

function pools({ numbers, symbols, unambiguous }) {
  const sets = [LOWER, UPPER, ...(numbers ? [DIGITS] : []), ...(symbols ? [SYMBOLS] : [])];
  return unambiguous ? sets.map((set) => set.replace(AMBIGUOUS, "")) : sets;
}

/** A password using every character set at least once. */
export function generatePassword(length, options) {
  const sets = pools(options);
  const all = sets.join("");
  const chars = sets.map((set) => set[randomIndex(set.length)]);
  while (chars.length < length) chars.push(all[randomIndex(all.length)]);
  // Shuffle so the guaranteed characters are not always first.
  for (let i = chars.length - 1; i > 0; i -= 1) {
    const j = randomIndex(i + 1);
    [chars[i], chars[j]] = [chars[j], chars[i]];
  }
  return chars.slice(0, length).join("");
}

function entropyBits(length, options) {
  return Math.round(length * Math.log2(pools(options).join("").length));
}

function strength(bits) {
  if (bits >= 100) return ["Very Strong", "success"];
  if (bits >= 72) return ["Strong", "success"];
  if (bits >= 50) return ["Fair", "warning"];
  return ["Weak", "danger"];
}

export default class GeneratePassword extends View {
  init() {
    const { preferences } = launch();
    const length = Number(preferences.length ?? 20);
    this.length = LENGTHS.includes(length) ? length : 20;
    this.options = {
      numbers: preferences.numbers !== false,
      symbols: preferences.symbols !== false,
      unambiguous: preferences.unambiguous === true,
    };
    this.generate();
  }

  generate() {
    this.passwords = Array.from({ length: COUNT }, () => generatePassword(this.length, this.options));
  }

  regenerate(cx) {
    this.generate();
    cx.notify();
  }

  row(password, index) {
    const bits = entropyBits(this.length, this.options);
    const [label, tone] = strength(bits);
    return new ListItem(`password-${index}`, password)
      .icon("key-round")
      .tag(label, tone)
      .accessory(`${bits} bits`)
      .accessory_tooltip("Entropy: how many guesses, as a power of two, a brute-force attack needs")
      .actions(
        new ActionPanel().children([
          new Action("Copy Password").icon("copy").copy(password),
          new Action("Paste Password").icon("clipboard-paste").paste(password),
          new Action("Generate New Passwords").icon("refresh-cw").shortcut("secondary-r").run((cx) => this.regenerate(cx)),
        ]),
      );
  }

  render() {
    return new List()
      .placeholder("Pick a length on the right; Enter copies")
      .filtering(false)
      .empty_title("No passwords")
      .dropdown(
        new ListDropdown("Length")
          .value(String(this.length))
          .children(LENGTHS.map((length) => new ListDropdownItem(String(length), `${length} Characters`)))
          .on_change((value, cx) => {
            this.length = Number(value);
            this.regenerate(cx);
          }),
      )
      .children(this.passwords.map((password, index) => this.row(password, index)));
  }
}
