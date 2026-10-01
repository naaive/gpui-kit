// JSON checks with a position: `JSON.parse` says that text is wrong but not
// reliably where, so a small scanner finds the first offending character and
// reports it as a line and a column.

/** Parses `text`; answers `{ value }` or `{ error: { message, line, column } }`. */
export function parseJson(text) {
  try {
    return { value: JSON.parse(text) };
  } catch (error) {
    const found = locateError(text) ?? { offset: text.length, message: String(error?.message ?? error) };
    return { error: { message: found.message, ...lineAndColumn(text, found.offset) } };
  }
}

export function lineAndColumn(text, offset) {
  const before = text.slice(0, offset).split("\n");
  return { line: before.length, column: before[before.length - 1].length + 1 };
}

/** Sorts object keys at every depth, for a stable, comparable form. */
export function sortKeys(value) {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value && typeof value === "object") {
    return Object.keys(value)
      .sort()
      .reduce((sorted, key) => ({ ...sorted, [key]: sortKeys(value[key]) }), {});
  }
  return value;
}

class Failure {
  constructor(offset, message) {
    this.offset = offset;
    this.message = message;
  }
}

/** The first error in `text` as `{ offset, message }`, or null if it is valid. */
export function locateError(text) {
  let at = 0;
  const fail = (message) => {
    const ended = at >= text.length && !message.startsWith("Unexpected end");
    throw new Failure(at, ended ? `Unexpected end of input: ${message.charAt(0).toLowerCase()}${message.slice(1)}` : message);
  };
  const describe = () => (at >= text.length ? "Unexpected end of input" : `Unexpected character “${text[at]}”`);
  const space = () => {
    while (at < text.length && " \t\n\r".includes(text[at])) at += 1;
  };
  const literal = (word) => {
    if (text.startsWith(word, at)) at += word.length;
    else fail(describe());
  };
  const string = () => {
    at += 1;
    while (at < text.length) {
      const char = text[at];
      if (char === '"') {
        at += 1;
        return;
      }
      if (char === "\\") {
        const escape = text[at + 1];
        if (escape === "u") {
          if (!/^[0-9a-fA-F]{4}$/.test(text.slice(at + 2, at + 6))) {
            at += 1;
            fail("Invalid \\u escape");
          }
          at += 6;
        } else if ('"\\/bfnrt'.includes(escape ?? "x")) {
          at += 2;
        } else {
          at += 1;
          fail("Invalid escape sequence");
        }
      } else if (char < " ") {
        fail("Control character in string");
      } else {
        at += 1;
      }
    }
    fail("Unterminated string");
  };
  const number = () => {
    const match = /^-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?/.exec(text.slice(at));
    if (!match) fail("Invalid number");
    at += match[0].length;
  };
  const value = () => {
    space();
    const char = text[at];
    if (char === "{") return object();
    if (char === "[") return array();
    if (char === '"') return string();
    if (char === "t") return literal("true");
    if (char === "f") return literal("false");
    if (char === "n") return literal("null");
    if (char === "-" || (char >= "0" && char <= "9")) return number();
    fail(describe());
  };
  const object = () => {
    at += 1;
    space();
    if (text[at] === "}") {
      at += 1;
      return;
    }
    for (;;) {
      space();
      if (text[at] !== '"') fail(text[at] === "}" ? "Trailing comma" : "Expected a property name in double quotes");
      string();
      space();
      if (text[at] !== ":") fail("Expected “:” after a property name");
      at += 1;
      value();
      space();
      if (text[at] === ",") {
        at += 1;
        continue;
      }
      if (text[at] === "}") {
        at += 1;
        return;
      }
      fail("Expected “,” or “}”");
    }
  };
  const array = () => {
    at += 1;
    space();
    if (text[at] === "]") {
      at += 1;
      return;
    }
    for (;;) {
      space();
      if (text[at] === "]") fail("Trailing comma");
      value();
      space();
      if (text[at] === ",") {
        at += 1;
        continue;
      }
      if (text[at] === "]") {
        at += 1;
        return;
      }
      fail("Expected “,” or “]”");
    }
  };
  try {
    value();
    space();
    if (at < text.length) fail("Unexpected text after the JSON value");
    return null;
  } catch (error) {
    if (error instanceof Failure) return { offset: error.offset, message: error.message };
    throw error;
  }
}
