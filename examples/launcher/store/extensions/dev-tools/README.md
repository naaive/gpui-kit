# Developer Tools

Everyday text utilities that work offline. Every command that takes text uses
what you type in the search field, or, until you type, the text that was
selected when you opened the launcher. Enter copies a result and Ctrl/Cmd-Enter
pastes it into the app you came from.

## Commands

- **Format JSON**: pretty-printed, minified and key-sorted JSON, or the line and
  column where invalid JSON breaks. Edit Input (Ctrl/Cmd-E) opens a form for
  longer JSON. The indentation is a command preference.
- **Encode or Decode Text**: Base64 (standard and URL-safe), URL encoding and
  HTML entities both ways; a JWT shows its header, payload and expiry. The
  signature is not verified.
- **Hash Text**: MD5, SHA-1, SHA-256, SHA-384 and SHA-512 as hex, or Base64
  with Ctrl/Cmd-Shift-C.
- **Generate UUID**: random version 4 UUIDs. Type how many (up to 100); the
  dropdown picks lowercase, uppercase, no hyphens or braces.
- **Generate Password**: passwords from `crypto.randomBytes`. The preferences set
  the default length and whether to use numbers, symbols and look-alike
  characters; the dropdown changes the length.
- **Convert Timestamp**: Unix seconds, milliseconds or microseconds, or an ISO
  8601 date, shown as every other form. Empty input shows the current time.
- **Lorem Ipsum**: paragraphs, sentences and words of placeholder text. Type a
  number to set how many.
- **Test Regex**: type `/pattern/flags text`, or a bare pattern to test it on
  the selected text. Each match is listed with its groups. Edit in Form
  (Ctrl/Cmd-E) is for longer text.
- **Count Characters**: characters, words, lines, sentences, paragraphs, UTF-8
  bytes and reading time.

## Permissions

None. The commands make no network requests and run no programs. Generate UUID
remembers its format in the extension's own storage.
