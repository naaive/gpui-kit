//! `launcher/utils`: plain JavaScript helpers, served from source.
//!
//! These helpers need no native capability, so they are JavaScript rather
//! than host functions: `Query` must call the view's `cx.notify()` after an
//! `await`, which a host function, holding no script value, could never do.
//! The source lives in `utils.js` beside this file so it can be read, linted
//! and tested as the JavaScript it is.

/// The specifier extensions import the helpers from.
pub const UTILS_MODULE: &str = "launcher/utils";

const SOURCE: &str = include_str!("utils.js");

const DECLARATIONS: &str = r#"
  /** Options of a `Query`. */
  export interface QueryOptions<T> {
    /** Caches the data under this key; the next launch shows it at once while it loads. */
    cache_key?: string;
    /** The data before the first load finishes. */
    initial?: T;
    /** Whether to start loading at once. Defaults to true. */
    execute?: boolean;
    /** The title of the toast shown when loading fails. */
    failure_title?: string;
    /** Set to false to show no toast on failure. */
    failure_toast?: boolean;
  }

  /** Loads data for a view and re-renders it when `data`, `loading` or `error` change. */
  export class Query<T> {
    /** `cx` is the context `init(props, cx)` receives. */
    constructor(cx: Context, loader: () => T | Promise<T>, options?: QueryOptions<T>);
    readonly data: T | null;
    readonly loading: boolean;
    readonly error: unknown;
    /** Loads again; a result overtaken by a newer load is dropped. */
    revalidate(cx?: Context): Promise<T | null>;
    /** Replaces the data without loading. */
    mutate(data: T): void;
  }

  /** One page of a `Paginator`. */
  export interface Page<T> {
    items: T[];
    has_more: boolean;
    /** Passed to the loader for the next page. */
    cursor?: unknown;
  }

  /** Loads a list one page at a time into `items`, for `List.on_load_more`. */
  export class Paginator<T> {
    constructor(
      cx: Context,
      loader: (page: number, cursor: unknown) => Page<T> | Promise<Page<T>>,
      options?: { execute?: boolean; failure_title?: string; failure_toast?: boolean },
    );
    readonly items: T[];
    readonly page: number;
    readonly has_more: boolean;
    readonly loading: boolean;
    readonly error: unknown;
    load_more(cx?: Context): Promise<T[]>;
    /** Starts again from the first page. */
    reset(cx?: Context): Promise<T[]>;
  }

  export type FormValue = string | boolean | null;
  /** Answers an error message, or nothing when the value is fine. */
  export type FormRule = (value: FormValue, values: { [id: string]: FormValue }) => string | undefined | null;

  /** The values and validation errors of a Form, kept by the view. */
  export class FormState {
    constructor(
      initial?: { [id: string]: FormValue },
      rules?: { [id: string]: FormRule | FormRule[] },
    );
    static required(message?: string): FormRule;
    readonly values: { [id: string]: FormValue };
    readonly errors: { [id: string]: string };
    value(id: string): FormValue;
    error(id: string): string | undefined;
    set(id: string, value: FormValue): void;
    /** Merges `values` in, runs every rule, and answers whether the form is valid. */
    validate(values?: { [id: string]: FormValue }): boolean;
    reset(values?: { [id: string]: FormValue }): void;
    /** Gives a field this state's value and error, and keeps the value when it changes. */
    bind<F>(id: string, field: F): F;
  }

  /** How often and how recently an item was used. */
  export interface Visit {
    count: number;
    /** Milliseconds since the epoch. */
    last: number;
  }

  /** Sorts items by use, most used first; unused items keep their order. */
  export function frecency_sort<T>(
    items: T[],
    key: (item: T) => string,
    visits: { [id: string]: Visit },
    now?: number,
  ): T[];

  /** Remembers which items were used, in the extension's cache. */
  export class Frecency {
    constructor(cache_key?: string);
    readonly visits: { [id: string]: Visit };
    visit(id: string, now?: number): void;
    sort<T>(items: T[], key: (item: T) => string, now?: number): T[];
  }
"#;

/// The source of `launcher/utils`, to serve as an ES module.
pub fn utils_module_source() -> &'static str {
    SOURCE
}

/// The body of `declare module "launcher/utils"`.
pub fn utils_declarations() -> &'static str {
    DECLARATIONS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A declaration for every export, so an editor never offers a helper
    /// that does not exist or hides one that does.
    #[test]
    fn test_declarations_name_every_export() {
        let exports: Vec<&str> = SOURCE
            .lines()
            .filter_map(|line| {
                let rest = line
                    .strip_prefix("export class ")
                    .or_else(|| line.strip_prefix("export function "))?;
                rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .next()
            })
            .collect();
        assert_eq!(
            exports,
            [
                "Query",
                "Paginator",
                "FormState",
                "frecency_sort",
                "Frecency"
            ]
        );
        for export in exports {
            assert!(
                DECLARATIONS.contains(&format!("export class {export}"))
                    || DECLARATIONS.contains(&format!("export function {export}")),
                "`{export}` is not declared"
            );
        }
    }
}
