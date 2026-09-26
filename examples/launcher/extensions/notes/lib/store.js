// Notes live in the extension's own `localStorage`. GPUI Shell grants every
// extension its storage without a capability, keyed by the extension id, so
// no other extension can read them.
const KEY = "notes";

export function loadNotes() {
  try {
    const notes = JSON.parse(localStorage.getItem(KEY) ?? "[]");
    return Array.isArray(notes) ? notes : [];
  } catch (_) {
    return [];
  }
}

function saveNotes(notes) {
  localStorage.setItem(KEY, JSON.stringify(notes));
}

/** Adds a note, or replaces the one with the same `id`, and moves it first. */
export function putNote({ id, title, body }) {
  const note = {
    id: id ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`,
    title,
    body,
    updated: Date.now(),
  };
  saveNotes([note, ...loadNotes().filter((other) => other.id !== note.id)]);
  return note;
}

export function deleteNote(id) {
  saveNotes(loadNotes().filter((note) => note.id !== id));
}
