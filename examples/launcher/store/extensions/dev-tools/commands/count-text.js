// Counts the characters, words, lines, sentences and paragraphs of the
// typed or selected text, and estimates how long it takes to read.
import { Buffer } from "buffer";
import { Action, ActionPanel, ListItem } from "launcher";
import { TextToolView } from "../lib/ui.js";

const WORDS_PER_MINUTE = 230;

/** Every count, as `[id, title, value]`. */
export function countText(text) {
  const characters = Array.from(text).length;
  const words = text.match(/[\p{L}\p{N}][\p{L}\p{N}'’_-]*/gu)?.length ?? 0;
  const lines = text === "" ? 0 : text.split(/\r\n|\r|\n/).length;
  const sentences = text.match(/[^.!?。！？]*[\p{L}\p{N}][^.!?。！？]*([.!?。！？]+|$)/gu)?.filter((s) => s.trim())
    .length ?? 0;
  const paragraphs = text.split(/(?:\r?\n\s*){2,}/).filter((part) => part.trim() !== "").length;
  const minutes = words / WORDS_PER_MINUTE;
  const reading = words === 0 ? "0 seconds" : minutes < 1 ? `${Math.max(1, Math.round(minutes * 60))} seconds` : `${Math.round(minutes)} min`;
  return [
    ["characters", "Characters", characters],
    ["no-spaces", "Characters Without Spaces", Array.from(text.replace(/\s/g, "")).length],
    ["words", "Words", words],
    ["lines", "Lines", lines],
    ["sentences", "Sentences", sentences],
    ["paragraphs", "Paragraphs", paragraphs],
    ["bytes", "Bytes (UTF-8)", Buffer.from(text, "utf8").length],
    ["reading", "Reading Time", reading],
  ];
}

const ICONS = {
  characters: "type",
  "no-spaces": "text-cursor",
  words: "whole-word",
  lines: "list",
  sentences: "text-quote",
  paragraphs: "pilcrow",
  bytes: "binary",
  reading: "clock",
};

export default class CountText extends TextToolView {
  page() {
    return {
      placeholder: "Type text to count…",
      empty_title: "Count Characters, Words and Lines",
      empty_description: "Type text, or select some before opening the launcher.",
      showing_detail: false,
    };
  }

  rows(input) {
    if (input === "") return [];
    const counts = countText(input);
    const report = counts.map(([, title, value]) => `${title}: ${value}`).join("\n");
    const source = this.from_selection ? "Selected text" : "Typed text";
    return counts.map(([id, title, value]) => {
      const item = new ListItem(id, String(value)).icon(ICONS[id]).subtitle(title);
      return (id === "characters" ? item.accessory(source) : item).actions(
        new ActionPanel().children([
          new Action("Copy Count").icon("copy").copy(String(value)),
          new Action("Copy All Counts").icon("copy").shortcut("secondary-shift-c").copy(report),
        ]),
      );
    });
  }
}
