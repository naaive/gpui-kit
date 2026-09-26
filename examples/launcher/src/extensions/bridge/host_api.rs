//! `launcher/api`: host functions for extensions.
//!
//! Host functions exchange plain data only and never hold a script value
//! (GPUI Shell §17.6), so everything here is a question the host answers or a
//! request it carries out after the call returns.
//!
//! Each extension gets its own module instance, built by
//! [`HostApi::module_for`] around an [`ExtensionContext`] and registered on
//! that extension's `Policy`. A host function cannot ask which extension is
//! calling it, so the context is how `launch()` knows the command, how the
//! cache knows its file, and how `launch_command("other")` knows whose
//! `other` is meant. [`HostApi::export`] serves the same functions process-wide
//! for a host that has not moved to per-extension policies yet; it answers for
//! whichever command was launched last.

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::Result;
use gpui_kit::{App, SharedString};
use gpui_shell::{HostArguments, HostError, HostModule, HostObject, HostValue};
use serde_json::{Map, Value};

use super::{
    cache::{self, Cache},
    components::parse_toast_style,
};
use crate::{
    extensions::{CommandId, LaunchRequest},
    model::{Effect, Toast, ToastStyle},
};

pub const MODULE: &str = "launcher/api";

/// The TypeScript face of `launcher/api`, checked against the registered
/// functions by `HostModule::validate`.
pub const DECLARATIONS: &str = r#"
/** A JSON value, as preferences and the cache hold them. */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

export interface Launch {
  /** The extension's id, from `gpui-shell.json`. */
  extension: string;
  /** The command's name, from `launcher.json`. */
  command: string;
  /** What the user typed into the command's arguments, by argument name. */
  arguments: { [name: string]: string };
  /** The extension's and the command's preferences, by preference name. */
  preferences: { [name: string]: Json };
  /** Whether the user opened the command or the launcher ran it on its own. */
  launch_type: "user_initiated";
}

export interface ToastOptions {
  title: string;
  message?: string;
  /** Defaults to `info`. A `progress` toast is replaced by the next one with the same `id`. */
  style?: "info" | "success" | "failure" | "progress";
  /** Toasts with the same id replace each other. */
  id?: string;
}

export interface Environment {
  appearance: "light" | "dark";
  /** A BCP 47 tag, such as `en` or `zh-CN`. */
  locale: string;
  launcher_version: string;
  /** True while the extension is loaded for development. */
  development: boolean;
}

/** The command this page was opened for, with its arguments and preferences. */
export function launch(): Launch;
/** Shows a message in the launcher. `show_toast(title, style)` is accepted too. */
export function show_toast(options: ToastOptions | string, style?: ToastOptions["style"]): void;
/** Hides the launcher and shows a short message. */
export function show_hud(text: string): void;
/** Hides the launcher. */
export function close_main_window(): void;
/** Returns to the previous page. */
export function pop(): void;
/** Returns to the root search. */
export function pop_to_root(): void;
/** Opens a URL in the default browser, or a file or folder with its default application. */
export function open(target: string): void;
/** Copies text to the clipboard. */
export function copy(text: string): void;
/** Pastes text into the application that was frontmost, then hides the launcher. */
export function paste(text: string): void;
/** Opens another command of this extension, or `extension-id/command` of another. */
export function launch_command(name: string, arguments?: { [name: string]: string }): void;
/** The launcher's appearance, locale and version. */
export function environment(): Environment;
/** A value this extension cached, or `null`. */
export function cache_get(key: string): Json;
/** Caches a JSON value. The whole cache is limited to 10 MB. */
export function cache_set(key: string, value: Json): void;
/** Removes one cached value; answers whether it was there. */
export function cache_remove(key: string): boolean;
/** Removes every cached value of this extension. */
export function cache_clear(): void;
/** Changes how this command appears in the root search, such as a subtitle showing unread count. `null` restores the manifest's. */
export function update_command_metadata(metadata: { subtitle?: string | null }): void;
"#;

/// Where requested effects go; the launcher window performs them.
pub type EffectSink = Rc<dyn Fn(Effect, &mut App)>;
/// Where metadata updates go; the root search shows them.
pub type MetadataSink = Rc<dyn Fn(CommandId, CommandMetadata, &mut App)>;

/// How a command was opened. The launcher runs nothing on its own schedule,
/// so every launch is the user's; the type leaves room for scheduled runs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LaunchType {
    #[default]
    UserInitiated,
}

impl LaunchType {
    fn as_str(self) -> &'static str {
        match self {
            Self::UserInitiated => "user_initiated",
        }
    }
}

/// What a command changed about how the root search shows it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandMetadata {
    subtitle: Option<SharedString>,
}

impl CommandMetadata {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the manifest's subtitle; `None` restores it.
    pub fn with_subtitle(mut self, subtitle: Option<SharedString>) -> Self {
        self.subtitle = subtitle;
        self
    }

    pub fn subtitle(&self) -> Option<&SharedString> {
        self.subtitle.as_ref()
    }
}

/// Everything `launcher/api` answers for one extension.
///
/// Cheap to clone, and clones share one state: the host keeps a clone after
/// handing one to [`HostApi::module_for`], and updates the launch with
/// [`Self::begin_launch`] before mounting each command, so one module instance
/// serves every command of the extension.
#[derive(Clone)]
pub struct ExtensionContext(Rc<ContextState>);

struct ContextState {
    extension: SharedString,
    launch: RefCell<Launch>,
    preferences: RefCell<Map<String, Value>>,
    locale: RefCell<SharedString>,
    development: Cell<bool>,
    cache: RefCell<Option<Cache>>,
    cache_directory: RefCell<Option<PathBuf>>,
    cache_limit: Cell<usize>,
    effects: RefCell<Option<EffectSink>>,
    metadata: RefCell<Option<MetadataSink>>,
}

#[derive(Clone, Default)]
struct Launch {
    command: Option<SharedString>,
    arguments: BTreeMap<SharedString, SharedString>,
    launch_type: LaunchType,
}

impl ExtensionContext {
    pub fn new(extension: impl Into<SharedString>) -> Self {
        Self(Rc::new(ContextState {
            extension: extension.into(),
            launch: RefCell::default(),
            preferences: RefCell::default(),
            locale: RefCell::new("en".into()),
            development: Cell::new(false),
            cache: RefCell::new(None),
            cache_directory: RefCell::new(None),
            cache_limit: Cell::new(cache::DEFAULT_LIMIT),
            effects: RefCell::new(None),
            metadata: RefCell::new(None),
        }))
    }

    /// The command `launch()` reports, before any [`Self::begin_launch`].
    pub fn with_command(self, command: impl Into<SharedString>) -> Self {
        self.0.launch.borrow_mut().command = Some(command.into());
        self
    }

    pub fn with_argument(
        self,
        name: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        self.0
            .launch
            .borrow_mut()
            .arguments
            .insert(name.into(), value.into());
        self
    }

    /// The resolved preference values, extension and command ones together,
    /// with defaults applied. Passwords belong here too: only this extension's
    /// module ever reads them.
    pub fn with_preferences(self, preferences: Map<String, Value>) -> Self {
        self.set_preferences(preferences);
        self
    }

    #[cfg(test)]
    pub fn with_preference(self, name: impl Into<String>, value: Value) -> Self {
        self.0.preferences.borrow_mut().insert(name.into(), value);
        self
    }

    pub fn with_locale(self, locale: impl Into<SharedString>) -> Self {
        self.0.locale.replace(locale.into());
        self
    }

    pub fn with_development(self, development: bool) -> Self {
        self.0.development.set(development);
        self
    }

    /// The directory holding this extension's cache file. Without one the
    /// cache functions refuse with a sentence saying so.
    pub fn with_cache_directory(self, directory: impl Into<PathBuf>) -> Self {
        self.0.cache_directory.replace(Some(directory.into()));
        self.0.cache.replace(None);
        self
    }

    pub fn with_effect_sink(self, sink: impl Fn(Effect, &mut App) + 'static) -> Self {
        self.set_effect_sink(Rc::new(sink));
        self
    }

    /// Receives `update_command_metadata` calls. The root search keeps the
    /// latest metadata per command and shows it instead of the manifest's.
    pub fn with_metadata_sink(
        self,
        sink: impl Fn(CommandId, CommandMetadata, &mut App) + 'static,
    ) -> Self {
        self.0.metadata.replace(Some(Rc::new(sink)));
        self
    }

    /// Records the command about to be mounted. Call it before
    /// `mount_application`, because a View reads `launch()` in `init`.
    #[cfg(test)]
    pub fn begin_launch(&self, request: &LaunchRequest, launch_type: LaunchType) {
        self.0.launch.replace(Launch {
            command: Some(request.command().command().clone()),
            arguments: request.arguments().clone(),
            launch_type,
        });
        super::note_launched(&self.0.extension);
    }

    pub fn set_preferences(&self, preferences: Map<String, Value>) {
        self.0.preferences.replace(preferences);
    }

    pub fn set_effect_sink(&self, sink: EffectSink) {
        self.0.effects.replace(Some(sink));
    }

    pub fn extension(&self) -> &SharedString {
        &self.0.extension
    }

    pub fn command(&self) -> Option<SharedString> {
        self.0.launch.borrow().command.clone()
    }

    pub fn locale(&self) -> SharedString {
        self.0.locale.borrow().clone()
    }

    pub fn is_development(&self) -> bool {
        self.0.development.get()
    }

    fn request(&self, effect: Effect) {
        let Some(sink) = self.0.effects.borrow().clone() else {
            tracing::info!("effect requested with no launcher window: {effect:?}");
            return;
        };
        gpui_shell::with_current_app(|cx| sink(effect, cx));
    }

    fn launch_value(&self) -> Result<HostValue, HostError> {
        let launch = self.0.launch.borrow();
        let command = launch
            .command
            .as_ref()
            .ok_or_else(|| HostError::new("launch() is only available to a command's view"))?;
        let arguments = launch
            .arguments
            .iter()
            .fold(HostObject::new(), |object, (name, value)| {
                object.field(name.to_string(), value.to_string())
            });
        Ok(HostObject::new()
            .field("extension", self.0.extension.to_string())
            .field("command", command.to_string())
            .field("arguments", arguments)
            .field(
                "preferences",
                json_to_host(&Value::Object(self.0.preferences.borrow().clone())),
            )
            .field("launch_type", launch.launch_type.as_str())
            .into())
    }

    fn environment_value(&self) -> HostValue {
        let dark = gpui_shell::with_current_app(|cx| {
            if cx.has_global::<gpui_kit::component::Theme>() {
                gpui_kit::component::Theme::global(cx).is_dark()
            } else {
                matches!(
                    cx.window_appearance(),
                    gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark
                )
            }
        })
        .unwrap_or(false);
        HostObject::new()
            .field("appearance", if dark { "dark" } else { "light" })
            .field("locale", self.locale().to_string())
            .field("launcher_version", env!("CARGO_PKG_VERSION"))
            .field("development", self.is_development())
            .into()
    }

    fn with_cache<R>(
        &self,
        function: &str,
        body: impl FnOnce(&mut Cache) -> Result<R, String>,
    ) -> Result<R, HostError> {
        let mut cache = self.0.cache.borrow_mut();
        if cache.is_none() {
            let directory = self.0.cache_directory.borrow().clone().ok_or_else(|| {
                HostError::new(format!(
                    "{function}: the launcher gave `{}` no cache directory",
                    self.0.extension
                ))
            })?;
            *cache = Some(Cache::new(&directory, self.0.cache_limit.get()));
        }
        body(cache.as_mut().expect("just created")).map_err(HostError::new)
    }

    fn update_metadata(&self, metadata: CommandMetadata) -> Result<(), HostError> {
        let command = self.command().ok_or_else(|| {
            HostError::new("update_command_metadata() is only available to a command")
        })?;
        let Some(sink) = self.0.metadata.borrow().clone() else {
            tracing::info!("command metadata updated with no root search: {metadata:?}");
            return Ok(());
        };
        let id = CommandId::new(self.0.extension.clone(), command);
        gpui_shell::with_current_app(|cx| sink(id, metadata, cx));
        Ok(())
    }
}

/// The `launcher/api` host module; each extension gets an instance of its own.
pub struct HostApi;

impl HostApi {
    /// The `launcher/api` module for one extension, to register on its
    /// `Policy` with `Policy::with_host_module`.
    pub fn module_for(context: ExtensionContext) -> HostModule {
        module(Rc::new(move || Ok(context.clone())))
    }
}

type ContextSource = Rc<dyn Fn() -> Result<ExtensionContext, HostError>>;

/// The one implementation behind both registration paths.
fn module(context: ContextSource) -> HostModule {
    let function =
        |name: &'static str,
         body: fn(&ExtensionContext, &HostArguments) -> Result<HostValue, HostError>| {
            let context = context.clone();
            (name, move |arguments: &HostArguments| {
                body(&context()?, arguments)
            })
        };
    [
        function("launch", |context, _| context.launch_value()),
        function("show_toast", |context, arguments| {
            context.request(Effect::ShowToast(parse_toast(arguments)?));
            Ok(HostValue::Null)
        }),
        function("show_hud", |context, arguments| {
            context.request(Effect::ShowHud(arguments.string(0)?.to_owned().into()));
            Ok(HostValue::Null)
        }),
        function("close_main_window", |context, _| {
            context.request(Effect::CloseWindow);
            Ok(HostValue::Null)
        }),
        function("pop", |context, _| {
            context.request(Effect::Pop);
            Ok(HostValue::Null)
        }),
        function("pop_to_root", |context, _| {
            context.request(Effect::PopToRoot);
            Ok(HostValue::Null)
        }),
        function("open", |context, arguments| {
            context.request(open_effect(arguments.string(0)?)?);
            Ok(HostValue::Null)
        }),
        function("copy", |context, arguments| {
            context.request(Effect::Copy(arguments.string(0)?.to_owned().into()));
            Ok(HostValue::Null)
        }),
        function("paste", |context, arguments| {
            context.request(Effect::Paste(arguments.string(0)?.to_owned().into()));
            Ok(HostValue::Null)
        }),
        function("launch_command", |context, arguments| {
            context.request(Effect::Launch(parse_launch_command(
                context.extension(),
                arguments,
            )?));
            Ok(HostValue::Null)
        }),
        function("environment", |context, _| Ok(context.environment_value())),
        function("cache_get", |context, arguments| {
            let key = arguments.string(0)?;
            context.with_cache("cache_get", |cache| {
                Ok(cache
                    .get(key)?
                    .map(|value| json_to_host(&value))
                    .unwrap_or(HostValue::Null))
            })
        }),
        function("cache_set", |context, arguments| {
            let key = arguments.string(0)?;
            let value = host_to_json(arguments.value(1)?);
            context.with_cache("cache_set", |cache| cache.set(key, value))?;
            Ok(HostValue::Null)
        }),
        function("cache_remove", |context, arguments| {
            let key = arguments.string(0)?;
            Ok(context
                .with_cache("cache_remove", |cache| cache.remove(key))?
                .into())
        }),
        function("cache_clear", |context, _| {
            context.with_cache("cache_clear", |cache| cache.clear())?;
            Ok(HostValue::Null)
        }),
        function("update_command_metadata", |context, arguments| {
            context.update_metadata(parse_metadata(arguments)?)?;
            Ok(HostValue::Null)
        }),
    ]
    .into_iter()
    .fold(HostModule::new(MODULE), |module, (name, body)| {
        module.function(name, body)
    })
    .declarations(DECLARATIONS)
}

/// `show_toast({ title, message?, style?, id? })`, or the older
/// `show_toast(title, style?)`.
fn parse_toast(arguments: &HostArguments) -> Result<Toast, HostError> {
    let style = |value: Option<&HostValue>| -> Result<ToastStyle, HostError> {
        match value {
            None | Some(HostValue::Null) => Ok(ToastStyle::Info),
            Some(HostValue::Str(style)) => parse_toast_style(style).map_err(HostError::new),
            Some(other) => Err(HostError::new(format!(
                "show_toast: `style` must be a string, not {}",
                other.describe()
            ))),
        }
    };
    let options = arguments.value(0)?;
    if let HostValue::Str(title) = options {
        return Ok(Toast::new(style(arguments.get(1))?, title.clone()));
    }
    if options.as_object().is_none() {
        return Err(HostError::new(format!(
            "show_toast expects {{ title, message?, style?, id? }} or a title, not {}",
            options.describe()
        )));
    }
    let text = |field: &str| -> Result<Option<String>, HostError> {
        match options.get(field) {
            None | Some(HostValue::Null) => Ok(None),
            Some(HostValue::Str(text)) => Ok(Some(text.clone())),
            Some(other) => Err(HostError::new(format!(
                "show_toast: `{field}` must be a string, not {}",
                other.describe()
            ))),
        }
    };
    let title = text("title")?
        .filter(|title| !title.trim().is_empty())
        .ok_or_else(|| HostError::new("show_toast: `title` is required"))?;
    let mut toast = Toast::new(style(options.get("style"))?, title);
    if let Some(message) = text("message")? {
        toast = toast.with_message(message);
    }
    if let Some(id) = text("id")? {
        toast = toast.with_id(id);
    }
    Ok(toast)
}

/// A URL when the target starts with a scheme, otherwise a path; `~/` is the
/// home directory, as a user would type it.
fn open_effect(target: &str) -> Result<Effect, HostError> {
    let target = target.trim();
    if target.is_empty() {
        return Err(HostError::new("open: the target must not be empty"));
    }
    if has_url_scheme(target) {
        return Ok(Effect::OpenUrl(target.to_owned().into()));
    }
    let path = match target.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|home| Path::new(&home).join(rest))
            .unwrap_or_else(|| PathBuf::from(target)),
        None => PathBuf::from(target),
    };
    Ok(Effect::OpenPath(path))
}

/// `https:`, `mailto:`, `vscode:`… but not a Windows drive such as `C:`.
fn has_url_scheme(target: &str) -> bool {
    let Some((scheme, _)) = target.split_once(':') else {
        return false;
    };
    scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn parse_launch_command(
    extension: &SharedString,
    arguments: &HostArguments,
) -> Result<LaunchRequest, HostError> {
    let name = arguments.string(0)?.trim();
    if name.is_empty() {
        return Err(HostError::new("launch_command: the name must not be empty"));
    }
    let id = match name.rsplit_once('/') {
        Some((extension, command)) => CommandId::new(extension.to_owned(), command.to_owned()),
        None => CommandId::new(extension.clone(), name.to_owned()),
    };
    let mut request = LaunchRequest::new(id);
    match arguments.get(1) {
        None | Some(HostValue::Null) => {}
        Some(HostValue::Object(fields)) => {
            for (field, value) in fields {
                let HostValue::Str(value) = value else {
                    return Err(HostError::new(format!(
                        "launch_command: argument `{field}` must be a string, not {}",
                        value.describe()
                    )));
                };
                request = request.with_argument(field.clone(), value.clone());
            }
        }
        Some(other) => {
            return Err(HostError::new(format!(
                "launch_command: arguments must be an object of strings, not {}",
                other.describe()
            )));
        }
    }
    Ok(request)
}

fn parse_metadata(arguments: &HostArguments) -> Result<CommandMetadata, HostError> {
    let metadata = arguments.value(0)?;
    let Some(fields) = metadata.as_object() else {
        return Err(HostError::new(format!(
            "update_command_metadata expects {{ subtitle }}, not {}",
            metadata.describe()
        )));
    };
    let mut result = CommandMetadata::new();
    for (field, value) in fields {
        match (field.as_str(), value) {
            ("subtitle", HostValue::Null) => result = result.with_subtitle(None),
            ("subtitle", HostValue::Str(text)) => {
                result = result.with_subtitle(Some(text.clone().into()))
            }
            ("subtitle", other) => {
                return Err(HostError::new(format!(
                    "update_command_metadata: `subtitle` must be a string or null, not {}",
                    other.describe()
                )));
            }
            (other, _) => {
                return Err(HostError::new(format!(
                    "update_command_metadata: unknown field `{other}`; it accepts `subtitle`"
                )));
            }
        }
    }
    Ok(result)
}

pub(super) fn json_to_host(value: &Value) -> HostValue {
    match value {
        Value::Null => HostValue::Null,
        Value::Bool(value) => HostValue::Bool(*value),
        Value::Number(number) => number
            .as_f64()
            .map(HostValue::Number)
            .unwrap_or(HostValue::Null),
        Value::String(text) => HostValue::Str(text.clone()),
        Value::Array(items) => HostValue::Array(items.iter().map(json_to_host).collect()),
        Value::Object(fields) => HostValue::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), json_to_host(value)))
                .collect(),
        ),
    }
}

/// A number that JSON cannot hold (`NaN`, infinity) becomes `null`, as
/// `JSON.stringify` makes it.
fn host_to_json(value: &HostValue) -> Value {
    match value {
        HostValue::Null => Value::Null,
        HostValue::Bool(value) => Value::Bool(*value),
        HostValue::Number(number) => serde_json::Number::from_f64(*number)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        HostValue::Str(text) => Value::String(text.clone()),
        HostValue::Array(items) => Value::Array(items.iter().map(host_to_json).collect()),
        HostValue::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), host_to_json(value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn arguments(values: impl IntoIterator<Item = HostValue>) -> HostArguments {
        HostArguments::new(values)
    }

    fn object(fields: &[(&str, HostValue)]) -> HostValue {
        HostValue::Object(
            fields
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        )
    }

    #[test]
    fn test_module_declares_every_function_it_registers() {
        HostApi::module_for(ExtensionContext::new("test.extension"))
            .validate()
            .unwrap();
    }

    #[test]
    fn test_show_toast_accepts_both_forms() {
        let toast = parse_toast(&arguments([
            HostValue::from("Saved"),
            HostValue::from("success"),
        ]))
        .unwrap();
        assert_eq!(toast.style(), ToastStyle::Success);
        assert_eq!(toast.title().as_ref(), "Saved");

        let toast = parse_toast(&arguments([object(&[
            ("title", "Syncing".into()),
            ("message", "3 of 10".into()),
            ("style", "progress".into()),
            ("id", "sync".into()),
        ])]))
        .unwrap();
        assert_eq!(toast.style(), ToastStyle::Progress);
        assert_eq!(toast.message().map(|m| m.as_ref()), Some("3 of 10"));
        assert_eq!(toast.id().map(|id| id.as_ref()), Some("sync"));

        let error = parse_toast(&arguments([object(&[
            ("title", "Hi".into()),
            ("style", "loud".into()),
        ])]))
        .unwrap_err();
        assert!(error.message().contains("unknown toast style"), "{error}");
        let error = parse_toast(&arguments([object(&[])])).unwrap_err();
        assert!(error.message().contains("`title` is required"), "{error}");
    }

    #[test]
    fn test_open_tells_urls_from_paths() {
        assert!(matches!(
            open_effect("https://gpui-kit.com").unwrap(),
            Effect::OpenUrl(url) if url.as_ref() == "https://gpui-kit.com"
        ));
        assert!(matches!(
            open_effect("mailto:a@b.c").unwrap(),
            Effect::OpenUrl(_)
        ));
        assert!(matches!(
            open_effect("/tmp/notes.txt").unwrap(),
            Effect::OpenPath(path) if path == Path::new("/tmp/notes.txt")
        ));
        assert!(matches!(
            open_effect("C:\\Users").unwrap(),
            Effect::OpenPath(_)
        ));
        assert!(open_effect(" ").is_err());
    }

    #[test]
    fn test_launch_command_resolves_names_against_the_extension() {
        let extension: SharedString = "com.example.notes".into();
        let request = parse_launch_command(
            &extension,
            &arguments([
                HostValue::from("search"),
                object(&[("query", "gpui".into())]),
            ]),
        )
        .unwrap();
        assert_eq!(
            request.command(),
            &CommandId::new("com.example.notes", "search")
        );
        assert_eq!(
            request.arguments().get("query").map(|value| value.as_ref()),
            Some("gpui")
        );

        let request =
            parse_launch_command(&extension, &arguments([HostValue::from("other.ext/open")]))
                .unwrap();
        assert_eq!(request.command(), &CommandId::new("other.ext", "open"));

        let error = parse_launch_command(
            &extension,
            &arguments([HostValue::from("search"), object(&[("page", 2.into())])]),
        )
        .unwrap_err();
        assert!(error.message().contains("must be a string"), "{error}");
    }

    #[test]
    fn test_launch_reports_command_arguments_and_preferences() {
        let context = ExtensionContext::new("com.example.github").with_preferences(
            json!({ "token": "secret", "limit": 20 })
                .as_object()
                .unwrap()
                .clone(),
        );
        assert!(context.launch_value().is_err(), "no command launched yet");
        context.begin_launch(
            &LaunchRequest::new(CommandId::new("com.example.github", "search"))
                .with_argument("query", "gpui"),
            LaunchType::UserInitiated,
        );
        let launch = context.launch_value().unwrap();
        assert_eq!(launch.get("command"), Some(&HostValue::from("search")));
        assert_eq!(
            launch.get("arguments").and_then(|a| a.get("query")),
            Some(&HostValue::from("gpui"))
        );
        assert_eq!(
            launch.get("preferences").and_then(|p| p.get("limit")),
            Some(&HostValue::Number(20.0))
        );
        assert_eq!(
            launch.get("launch_type"),
            Some(&HostValue::from("user_initiated"))
        );
    }

    #[test]
    fn test_update_command_metadata_parses_subtitle() {
        let metadata =
            parse_metadata(&arguments([object(&[("subtitle", "3 unread".into())])])).unwrap();
        assert_eq!(metadata.subtitle().map(|s| s.as_ref()), Some("3 unread"));
        let metadata =
            parse_metadata(&arguments([object(&[("subtitle", HostValue::Null)])])).unwrap();
        assert_eq!(metadata.subtitle(), None);
        assert!(
            parse_metadata(&arguments([object(&[("title", "x".into())])]))
                .unwrap_err()
                .message()
                .contains("unknown field `title`")
        );
    }

    #[test]
    fn test_cache_functions_need_a_directory() {
        let context = ExtensionContext::new("com.example.nocache");
        let error = context
            .with_cache("cache_get", |cache| cache.get("key"))
            .unwrap_err();
        assert!(error.message().contains("no cache directory"), "{error}");
    }
}
