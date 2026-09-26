// launcher/utils: plain JavaScript helpers for class-based views.
//
// GPUI Shell views are classes with no automatic dependency tracking: a view
// changes its fields and calls `cx.notify()`. These helpers are objects a view
// keeps in a field, and they call `cx.notify()` themselves when their state
// changes, so a view reads `this.repos.data` in `render` and never writes the
// loading/error bookkeeping by hand.
//
// A helper needs a context to notify with. Give it the `cx` that `init(props,
// cx)` receives: that one is made to outlive the call (GPUI Shell §12.2), and
// the work it spawns belongs to the view whose call is running, so the view
// that created the helper is the one re-rendered. A call-scoped `cx` from an
// event handler works for one call, which is why every method that starts work
// also accepts the handler's `cx`.

import { cache_get, cache_set, show_toast } from "launcher/api";

function messageOf(error) {
  if (error === null || error === undefined) return "Unknown error";
  if (typeof error === "string") return error;
  return String(error.message ?? error);
}

function requireContext(cx, api) {
  if (!cx || typeof cx.spawn !== "function") {
    throw new TypeError(`${api} needs the \`cx\` that \`init(props, cx)\` receives`);
  }
  return cx;
}

// `notify` is refused while a view renders. A helper started from `render`
// still loads; the result arrives through the notify after the `await`.
function tryNotify(cx) {
  try {
    cx.notify();
  } catch (_) {
    // Rendering: the page is being drawn right now anyway.
  }
}

function readCache(key) {
  if (!key) return null;
  try {
    return cache_get(key);
  } catch (error) {
    console.warn(`launcher/utils: cannot read cache \`${key}\`: ${messageOf(error)}`);
    return null;
  }
}

function writeCache(key, value) {
  if (!key) return;
  try {
    cache_set(key, value === undefined ? null : value);
  } catch (error) {
    console.warn(`launcher/utils: cannot cache \`${key}\`: ${messageOf(error)}`);
  }
}

/**
 * Loads data for a view, keeps `data`, `loading` and `error`, and re-renders
 * the view whenever they change.
 *
 *   init(props, cx) {
 *     this.repos = new Query(cx, () => fetch(url).then((r) => r.json()), {
 *       cache_key: "repos",
 *     });
 *   }
 *   render() {
 *     return new List().loading(this.repos.loading).children(...);
 *   }
 */
export class Query {
  constructor(cx, loader, options = {}) {
    if (typeof loader !== "function") {
      throw new TypeError("new Query(cx, loader) expects a function returning the data or a promise of it");
    }
    this.cx = requireContext(cx, "new Query");
    this.loader = loader;
    this.options = options;
    this.error = null;
    this.loading = false;
    this.generation = 0;
    const cached = readCache(options.cache_key);
    this.data = cached !== null ? cached : options.initial ?? null;
    if (options.execute !== false) this.revalidate();
  }

  /**
   * Loads again. A result that arrives after a newer `revalidate` is dropped,
   * so a slow response never overwrites a fresh one. Resolves with the data.
   */
  revalidate(cx = this.cx) {
    const run = ++this.generation;
    this.loading = true;
    return new Promise((resolve) => {
      requireContext(cx, "Query.revalidate").spawn(async (task) => {
        tryNotify(task);
        try {
          const data = await this.loader();
          if (run !== this.generation) return;
          this.data = data;
          this.error = null;
          writeCache(this.options.cache_key, data);
        } catch (error) {
          if (run !== this.generation) return;
          this.error = error;
          if (this.options.failure_toast !== false) {
            show_toast({
              title: this.options.failure_title ?? "Failed to load",
              message: messageOf(error),
              style: "failure",
            });
          }
        } finally {
          if (run === this.generation) {
            this.loading = false;
            task.notify();
          }
          resolve(this.data);
        }
      });
    });
  }

  /** Replaces the data without loading, such as after a local edit. */
  mutate(data) {
    this.data = data;
    writeCache(this.options.cache_key, data);
  }
}

/**
 * Loads a list one page at a time into one array, for `List.on_load_more`.
 *
 * `loader(page, cursor)` answers `{ items, has_more, cursor? }`; `page`
 * counts from 0 and `cursor` is whatever the previous page returned.
 */
export class Paginator {
  constructor(cx, loader, options = {}) {
    if (typeof loader !== "function") {
      throw new TypeError("new Paginator(cx, loader) expects a function (page, cursor) => { items, has_more }");
    }
    this.cx = requireContext(cx, "new Paginator");
    this.loader = loader;
    this.options = options;
    this.generation = 0;
    this.clear();
    if (options.execute !== false) this.load_more();
  }

  clear() {
    this.items = [];
    this.page = 0;
    this.cursor = null;
    this.has_more = true;
    this.loading = false;
    this.error = null;
  }

  /** Loads the next page, unless one is loading or there are no more. */
  load_more(cx = this.cx) {
    if (this.loading || !this.has_more) return Promise.resolve(this.items);
    const run = this.generation;
    this.loading = true;
    return new Promise((resolve) => {
      requireContext(cx, "Paginator.load_more").spawn(async (task) => {
        tryNotify(task);
        try {
          const result = (await this.loader(this.page, this.cursor)) ?? {};
          if (run !== this.generation) return;
          const items = Array.isArray(result.items) ? result.items : [];
          this.items = this.items.concat(items);
          this.page += 1;
          this.cursor = result.cursor ?? null;
          this.has_more = Boolean(result.has_more) && items.length > 0;
          this.error = null;
        } catch (error) {
          if (run !== this.generation) return;
          this.error = error;
          this.has_more = false;
          if (this.options.failure_toast !== false) {
            show_toast({
              title: this.options.failure_title ?? "Failed to load more",
              message: messageOf(error),
              style: "failure",
            });
          }
        } finally {
          if (run === this.generation) {
            this.loading = false;
            task.notify();
          }
          resolve(this.items);
        }
      });
    });
  }

  /** Starts again from the first page, such as after the query changed. */
  reset(cx = this.cx) {
    this.generation += 1;
    this.clear();
    return this.load_more(cx);
  }
}

/**
 * The values and validation errors of a Form, kept by the view.
 *
 *   this.form = new FormState({ title: "" }, { title: FormState.required("Give it a title") });
 *   render() {
 *     return new Form().child(this.form.bind("title", new TextField("title", "Title")));
 *   }
 *   save(values, cx) {
 *     if (!this.form.validate(values)) return cx.notify();
 *   }
 */
export class FormState {
  constructor(initial = {}, rules = {}) {
    this.initial = { ...initial };
    this.rules = rules;
    this.values = { ...initial };
    this.errors = {};
  }

  /** A rule failing on an empty string, `null` or an unchecked checkbox. */
  static required(message = "Required") {
    return (value) =>
      value === null ||
      value === undefined ||
      value === false ||
      (typeof value === "string" && value.trim() === "")
        ? message
        : undefined;
  }

  value(id) {
    return this.values[id] ?? null;
  }

  error(id) {
    return this.errors[id];
  }

  /** Records a changed value; an error on that field is checked again. */
  set(id, value) {
    this.values[id] = value;
    if (id in this.errors) {
      const message = this.check(id);
      if (message) this.errors[id] = message;
      else delete this.errors[id];
    }
  }

  /** Runs every rule; answers whether the form is valid. */
  validate(values = {}) {
    this.values = { ...this.values, ...values };
    this.errors = {};
    for (const id of Object.keys(this.rules)) {
      const message = this.check(id);
      if (message) this.errors[id] = message;
    }
    return Object.keys(this.errors).length === 0;
  }

  reset(values = this.initial) {
    this.values = { ...values };
    this.errors = {};
  }

  /**
   * Gives a field this state's value and error, and keeps the value when the
   * user changes it.
   */
  bind(id, field) {
    const value = this.values[id];
    const bound = value === undefined || value === null ? field : field.value(value);
    const error = this.errors[id];
    return (error ? bound.error(error) : bound).on_change((next, cx) => {
      this.set(id, next);
      cx.notify();
    });
  }

  check(id) {
    const rules = this.rules[id];
    for (const rule of Array.isArray(rules) ? rules : [rules]) {
      if (typeof rule !== "function") continue;
      const message = rule(this.values[id] ?? null, this.values);
      if (message) return String(message);
    }
    return undefined;
  }
}

/** How long a visit takes to count half as much. */
const HALF_LIFE_MS = 3 * 24 * 60 * 60 * 1000;

function frecencyScore(visit, now) {
  if (!visit) return 0;
  const age = Math.max(0, now - (visit.last ?? 0));
  return (visit.count ?? 0) * Math.pow(0.5, age / HALF_LIFE_MS);
}

/**
 * Sorts `items` by how often and how recently each was used, most used first;
 * items never used keep their order after the rest. `visits` maps an id to
 * `{ count, last }`, as `Frecency` keeps it.
 */
export function frecency_sort(items, key, visits, now = Date.now()) {
  return items
    .map((item, index) => ({ item, index, score: frecencyScore(visits?.[key(item)], now) }))
    .sort((a, b) => b.score - a.score || a.index - b.index)
    .map((entry) => entry.item);
}

/** Remembers which items were used, in the extension's cache. */
export class Frecency {
  constructor(cache_key = "frecency") {
    this.cache_key = cache_key;
    this.visits = readCache(cache_key) ?? {};
  }

  visit(id, now = Date.now()) {
    const visit = this.visits[id] ?? { count: 0, last: 0 };
    this.visits[id] = { count: visit.count + 1, last: now };
    writeCache(this.cache_key, this.visits);
  }

  sort(items, key, now = Date.now()) {
    return frecency_sort(items, key, this.visits, now);
  }
}
