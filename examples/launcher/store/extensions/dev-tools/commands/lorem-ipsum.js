// Generates placeholder text. Typing a number sets how many paragraphs,
// sentences or words each row makes; Enter copies, Ctrl/Cmd-Enter pastes.
import { View } from "gpui-kit";
import { Action, List } from "launcher";
import { resultItem } from "../lib/ui.js";

const WORDS = (
  "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore " +
  "magna aliqua enim ad minim veniam quis nostrud exercitation ullamco laboris nisi aliquip ex ea commodo " +
  "consequat duis aute irure in reprehenderit voluptate velit esse cillum fugiat nulla pariatur excepteur sint " +
  "occaecat cupidatat non proident sunt culpa qui officia deserunt mollit anim id est laborum curabitur pretium " +
  "tincidunt lacus nulla gravida orci a odio nullam varius turpis et commodo pharetra est eros bibendum elit nec " +
  "luctus magna felis sollicitudin mauris integer in mauris eu nibh euismod gravida praesent blandit"
).split(" ");
const OPENING = "Lorem ipsum dolor sit amet, consectetur adipiscing elit";
const DEFAULT_COUNT = { paragraphs: 3, sentences: 5, words: 50 };
const MAX_COUNT = 500;

function between(low, high) {
  return low + Math.floor(Math.random() * (high - low + 1));
}

function words(count) {
  return Array.from({ length: count }, () => WORDS[between(0, WORDS.length - 1)]);
}

function capitalize(text) {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function sentence() {
  const parts = words(between(6, 14));
  if (parts.length > 8) parts[between(3, parts.length - 4)] += ",";
  return `${capitalize(parts.join(" "))}.`;
}

/** `count` sentences; the first starts with the classic opening. */
export function sentences(count) {
  return [`${OPENING}.`, ...Array.from({ length: Math.max(0, count - 1) }, sentence)].join(" ");
}

export function paragraphs(count) {
  return Array.from({ length: count }, (_, index) => {
    const body = Array.from({ length: between(4, 7) }, sentence).join(" ");
    return index === 0 ? `${OPENING}. ${body}` : body;
  }).join("\n\n");
}

export function wordList(count) {
  const list = ["lorem", "ipsum", ...words(Math.max(0, count - 2))].slice(0, count);
  return `${capitalize(list.join(" "))}.`;
}

export default class LoremIpsum extends View {
  init() {
    this.count = null;
    this.generate();
  }

  countOf(unit) {
    return this.count ?? DEFAULT_COUNT[unit];
  }

  generate() {
    this.texts = {
      paragraphs: paragraphs(this.countOf("paragraphs")),
      sentences: sentences(this.countOf("sentences")),
      words: wordList(this.countOf("words")),
    };
  }

  typed(query, cx) {
    const number = parseInt(query, 10);
    const count = Number.isFinite(number) && number > 0 ? Math.min(number, MAX_COUNT) : null;
    if (count !== this.count) {
      this.count = count;
      this.generate();
      cx.notify();
    }
  }

  row(unit, singular, icon) {
    const count = this.countOf(unit);
    const title = `${count} ${count === 1 ? singular : unit.charAt(0).toUpperCase() + unit.slice(1)}`;
    return resultItem(unit, title, this.texts[unit], {
      icon,
      accessory: `${this.texts[unit].length} characters`,
      extra: [
        new Action("Generate New Text")
          .icon("refresh-cw")
          .shortcut("secondary-r")
          .run((cx) => {
            this.generate();
            cx.notify();
          }),
      ],
    });
  }

  render() {
    return new List()
      .placeholder("How many? Type a number…")
      .showing_detail()
      .empty_title("No text")
      .on_query_change((query, cx) => this.typed(query, cx))
      .children([
        this.row("paragraphs", "Paragraph", "pilcrow"),
        this.row("sentences", "Sentence", "text-quote"),
        this.row("words", "Word", "whole-word"),
      ]);
  }
}
