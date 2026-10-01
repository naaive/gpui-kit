//! `Action` and the panel that groups actions: `ActionPanel`,
//! `ActionPanelSection`, `ActionPanelSubmenu`.
//!
//! An action carries exactly one effect. Everything but `run`, `submit` and
//! `push` is performed by the launcher without calling back into the script,
//! which is why opening a link or copying text needs no capability.

use anyhow::{Context as _, Result, bail};
use gpui_kit::{AnyElement, SharedString};
use gpui_shell::{
    ArgumentDescriptor, ArgumentSchema, ComponentArgument, ComponentDescriptor,
    ComponentMaterializer, ComponentPayload, MaterializeRequest, MethodDescriptor,
};

use super::shared::{
    RUN_CALLBACK, TEXT_CALLBACK, carry, describe, empty_constructor, form_values_data,
    optional_string, payload, recorded, reject_style, reporting, resolved, run_handler,
    string_method, strings_constructor, taken, text_handler, unit_method,
};
use crate::{
    extensions::{CommandId, LaunchRequest, bridge::current_extension, host::page_from_callback},
    model::{
        Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Confirmation, Effect,
        FormHandler, Image, PushHandler, Submenu, Toast, ToastStyle,
    },
};

const SUBMIT_CALLBACK: &str =
    "(values: { [id: string]: string | boolean | null }, cx: Context) => void";
const PUSH_CALLBACK: &str = "(cx: Context) => View";
const TOAST_STYLES: &[&str] = &["info", "success", "failure", "progress"];

pub(super) fn register(
    registry: &mut gpui_shell::ComponentRegistry,
) -> Result<(), gpui_shell::RegistryError> {
    registry.register(action_panel())?;
    registry.register(action_panel_section())?;
    registry.register(action_panel_submenu())?;
    registry.register(action())?;
    Ok(())
}

// MARK: ActionPanel

fn action_panel() -> ComponentDescriptor {
    ComponentDescriptor::new("ActionPanel", reporting(PanelMaterializer))
        .with_documentation(
            "The actions of an item, a detail or a form, shown by Cmd-K. Children are \
             `Action`, `ActionPanelSection` and `ActionPanelSubmenu`; the first action is the \
             primary one (Enter) and the second the secondary one (Cmd/Ctrl-Enter).",
        )
        .with_constructors(vec![empty_constructor("ActionPanel")])
}

struct PanelMaterializer;

impl ComponentMaterializer for PanelMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "ActionPanel")?;
        let mut panel = ActionPanel::new();
        for mut child in request.take_typed_children()? {
            let name = child.component_name();
            let mut element = request.materialize_child(&mut child)?;
            panel = match name {
                Some("Action") => panel.with_action(taken(&mut element, "an Action")?),
                Some("ActionPanelSection") => {
                    panel.with_section(taken(&mut element, "an ActionPanelSection")?)
                }
                Some("ActionPanelSubmenu") => {
                    panel.with_submenu(taken(&mut element, "an ActionPanelSubmenu")?)
                }
                other => bail!(
                    "ActionPanel accepts Action, ActionPanelSection and ActionPanelSubmenu \
                     children, not {}",
                    describe(other)
                ),
            };
        }
        Ok(carry(panel))
    }
}

/// Reads the `actions(panel)` argument of an item, a detail or a form.
pub(super) fn resolve_panel(
    request: &mut MaterializeRequest<'_>,
    argument: &ComponentArgument,
    owner: &str,
) -> Result<ActionPanel> {
    resolved(request, argument, "an ActionPanel")
        .with_context(|| format!("{owner}.actions expects an ActionPanel"))
}

/// Reads the `action(action)` shorthand argument.
pub(super) fn resolve_action(
    request: &mut MaterializeRequest<'_>,
    argument: &ComponentArgument,
    owner: &str,
) -> Result<Action> {
    resolved(request, argument, "an Action")
        .with_context(|| format!("{owner}.action expects an Action"))
}

// MARK: ActionPanelSection

#[derive(Clone)]
struct SectionTitle(Option<String>);

fn action_panel_section() -> ComponentDescriptor {
    ComponentDescriptor::new("ActionPanelSection", reporting(SectionMaterializer))
        .with_documentation(
            "A group of actions separated from the others, optionally titled. Children are \
             `Action` and `ActionPanelSubmenu`.",
        )
        .with_constructors(vec![gpui_shell::ConstructorDescriptor::new(
            "ActionPanelSection",
            vec![ArgumentDescriptor::new(
                "title",
                ArgumentSchema::Optional(Box::new(ArgumentSchema::String)),
            )],
            |arguments| match arguments {
                [title] => Ok(ComponentPayload::new(SectionTitle(
                    optional_string(title).filter(|title| !title.trim().is_empty()),
                ))),
                _ => Err("ActionPanelSection expects an optional title".into()),
            },
        )])
}

struct SectionMaterializer;

impl ComponentMaterializer for SectionMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "ActionPanelSection")?;
        let SectionTitle(title) = payload(&request, "ActionPanelSection")?;
        let mut section = ActionSection::new();
        if let Some(title) = title {
            section = section.with_title(title);
        }
        for mut child in request.take_typed_children()? {
            let name = child.component_name();
            let mut element = request.materialize_child(&mut child)?;
            let entry = match name {
                Some("Action") => ActionEntry::Action(taken(&mut element, "an Action")?),
                Some("ActionPanelSubmenu") => {
                    ActionEntry::Submenu(taken(&mut element, "an ActionPanelSubmenu")?)
                }
                other => bail!(
                    "ActionPanelSection accepts Action and ActionPanelSubmenu children, not {}",
                    describe(other)
                ),
            };
            section = section.with_entry(entry);
        }
        Ok(carry(section))
    }
}

// MARK: ActionPanelSubmenu

#[derive(Clone)]
struct Title(String);

#[derive(Clone)]
enum SubmenuOp {
    Icon(String),
    Shortcut(String),
}

fn action_panel_submenu() -> ComponentDescriptor {
    ComponentDescriptor::new("ActionPanelSubmenu", reporting(SubmenuMaterializer))
        .with_documentation(
            "A nested list of actions, such as \"Set Priority\", opened from the action panel. \
             Children are `Action`.",
        )
        .with_constructors(vec![strings_constructor(
            "ActionPanelSubmenu",
            &["title"],
            &["title"],
            |mut values| Title(values.remove(0)),
        )])
        .with_methods(vec![
            string_method(
                "icon",
                "A Lucide icon name, or a path to an image inside the extension.",
                SubmenuOp::Icon,
            ),
            string_method(
                "shortcut",
                "A keystroke that opens the submenu, such as `cmd-shift-p`.",
                SubmenuOp::Shortcut,
            ),
        ])
}

struct SubmenuMaterializer;

impl ComponentMaterializer for SubmenuMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "ActionPanelSubmenu")?;
        let Title(title) = payload(&request, "ActionPanelSubmenu")?;
        let mut submenu = Submenu::new(title);
        for op in recorded::<SubmenuOp>(&request) {
            submenu = match op {
                SubmenuOp::Icon(icon) => submenu.with_image(Image::parse(&icon)),
                SubmenuOp::Shortcut(shortcut) => submenu.with_shortcut(shortcut),
            };
        }
        for mut child in request.take_typed_children()? {
            let name = child.component_name();
            let mut element = request.materialize_child(&mut child)?;
            match name {
                Some("Action") => submenu = submenu.with_action(taken(&mut element, "an Action")?),
                other => bail!(
                    "ActionPanelSubmenu accepts Action children, not {}",
                    describe(other)
                ),
            }
        }
        Ok(carry(submenu))
    }
}

// MARK: Action

#[derive(Clone)]
enum ActionOp {
    Icon(String),
    Shortcut(String),
    Destructive,
    Confirm {
        title: String,
        message: Option<String>,
    },
    Argument {
        name: String,
        value: String,
    },
    Effect(EffectOp),
}

/// One of the effects an action may carry; exactly one is allowed.
#[derive(Clone)]
enum EffectOp {
    OpenUrl(String),
    Open(String),
    Reveal(String),
    OpenWith {
        target: String,
        application: String,
    },
    Trash(Vec<String>),
    QuickLook(String),
    CreateQuicklink {
        name: String,
        link: String,
    },
    CreateSnippet {
        text: String,
        name: Option<String>,
    },
    PickDate {
        callback: ComponentArgument,
        include_time: bool,
    },
    Copy(String),
    Paste(String),
    Toast {
        title: String,
        style: ToastStyle,
        message: Option<String>,
    },
    Hud(String),
    Run(ComponentArgument),
    Submit(ComponentArgument),
    Push {
        callback: ComponentArgument,
        title: Option<String>,
    },
    Launch(String),
    Pop,
    PopToRoot,
    CloseWindow,
}

impl EffectOp {
    fn method(&self) -> &'static str {
        match self {
            Self::OpenUrl(_) => "open_url",
            Self::Open(_) => "open",
            Self::Reveal(_) => "reveal",
            Self::OpenWith { .. } => "open_with",
            Self::Trash(_) => "trash",
            Self::QuickLook(_) => "quick_look",
            Self::CreateQuicklink { .. } => "create_quicklink",
            Self::CreateSnippet { .. } => "create_snippet",
            Self::PickDate { .. } => "pick_date",
            Self::Copy(_) => "copy",
            Self::Paste(_) => "paste",
            Self::Toast { .. } => "toast",
            Self::Hud(_) => "hud",
            Self::Run(_) => "run",
            Self::Submit(_) => "submit",
            Self::Push { .. } => "push",
            Self::Launch(_) => "launch",
            Self::Pop => "pop",
            Self::PopToRoot => "pop_to_root",
            Self::CloseWindow => "close_window",
        }
    }
}

const EFFECT_METHODS: &str = "open_url, open, reveal, open_with, trash, quick_look, \
                              create_quicklink, create_snippet, pick_date, copy, paste, toast, \
                              hud, run, submit, push, launch, pop, pop_to_root or close_window";

fn effect_string(
    name: &'static str,
    documentation: &'static str,
    op: fn(String) -> EffectOp,
) -> MethodDescriptor {
    MethodDescriptor::new(
        name,
        vec![ArgumentDescriptor::new("value", ArgumentSchema::String)],
        move |arguments| match arguments {
            [ComponentArgument::String(value)] if !value.trim().is_empty() => {
                Ok(ComponentPayload::new(ActionOp::Effect(op(value.clone()))))
            }
            _ => Err(format!("{name} expects a non-empty string")),
        },
    )
    .with_documentation(documentation)
}

fn effect_callback(
    name: &'static str,
    documentation: &'static str,
    signature: &'static str,
    op: fn(ComponentArgument) -> EffectOp,
) -> MethodDescriptor {
    MethodDescriptor::new(
        name,
        vec![ArgumentDescriptor::new(
            "callback",
            ArgumentSchema::Callback(signature),
        )],
        move |arguments| match arguments {
            [callback @ ComponentArgument::Callback(_)] => Ok(ComponentPayload::new(
                ActionOp::Effect(op(callback.clone())),
            )),
            _ => Err(format!("{name} expects a function")),
        },
    )
    .with_documentation(documentation)
}

fn action() -> ComponentDescriptor {
    ComponentDescriptor::new("Action", reporting(ActionMaterializer))
        .with_documentation(
            "Something the user can do. Give it exactly one effect; `icon`, `shortcut`, \
             `destructive` and `confirm` refine it.",
        )
        .with_constructors(vec![strings_constructor(
            "Action",
            &["title"],
            &["title"],
            |mut values| Title(values.remove(0)),
        )])
        .with_methods(vec![
            effect_string(
                "open_url",
                "Opens a URL in the default browser.",
                EffectOp::OpenUrl,
            ),
            effect_string(
                "open",
                "Opens a file, folder or application with the system default.",
                EffectOp::Open,
            ),
            effect_string(
                "reveal",
                "Shows a file in the file manager.",
                EffectOp::Reveal,
            ),
            MethodDescriptor::new(
                "open_with",
                vec![
                    ArgumentDescriptor::new("target", ArgumentSchema::String),
                    ArgumentDescriptor::new("application", ArgumentSchema::String),
                ],
                |arguments| match arguments {
                    [
                        ComponentArgument::String(target),
                        ComponentArgument::String(application),
                    ] if !target.trim().is_empty() && !application.trim().is_empty() => Ok(
                        ComponentPayload::new(ActionOp::Effect(EffectOp::OpenWith {
                            target: target.clone(),
                            application: application.clone(),
                        })),
                    ),
                    _ => Err("open_with expects a target and an application".into()),
                },
            )
            .with_documentation(
                "Opens a file, folder or URL with an application: a path to it, or its name \
                 as the system knows it, such as `notepad` or `Safari`.",
            ),
            MethodDescriptor::new(
                "trash",
                vec![ArgumentDescriptor::new(
                    "paths",
                    ArgumentSchema::Array(Box::new(ArgumentSchema::String)),
                )],
                |arguments| match arguments {
                    [ComponentArgument::Array(paths)] if !paths.is_empty() => {
                        let paths = paths
                            .iter()
                            .map(|path| match path {
                                ComponentArgument::String(path) if !path.trim().is_empty() => {
                                    Ok(path.clone())
                                }
                                _ => Err("trash expects paths as non-empty strings".to_owned()),
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(ComponentPayload::new(ActionOp::Effect(EffectOp::Trash(
                            paths,
                        ))))
                    }
                    _ => Err("trash expects a non-empty array of paths".into()),
                },
            )
            .with_documentation(
                "Moves files or folders to the Trash (the Recycle Bin on Windows), where the \
                 user can put them back. Performed by the launcher, so it needs no `fs` grant.",
            ),
            effect_string(
                "quick_look",
                "Shows a file large: an image, text, or the system's preview of a document.",
                EffectOp::QuickLook,
            ),
            MethodDescriptor::new(
                "create_quicklink",
                vec![
                    ArgumentDescriptor::new("name", ArgumentSchema::String),
                    ArgumentDescriptor::new("link", ArgumentSchema::String),
                ],
                |arguments| match arguments {
                    [
                        ComponentArgument::String(name),
                        ComponentArgument::String(link),
                    ] if !link.trim().is_empty() => Ok(ComponentPayload::new(ActionOp::Effect(
                        EffectOp::CreateQuicklink {
                            name: name.clone(),
                            link: link.clone(),
                        },
                    ))),
                    _ => Err("create_quicklink expects a name and a non-empty link".into()),
                },
            )
            .with_documentation(
                "Opens Create Quicklink filled in with a name and a link, which may hold \
                 `{argument}`; the user saves it.",
            ),
            MethodDescriptor::new(
                "create_snippet",
                vec![
                    ArgumentDescriptor::new("text", ArgumentSchema::String),
                    ArgumentDescriptor::new(
                        "name",
                        ArgumentSchema::Optional(Box::new(ArgumentSchema::String)),
                    ),
                ],
                |arguments| match arguments {
                    [ComponentArgument::String(text), name] if !text.trim().is_empty() => Ok(
                        ComponentPayload::new(ActionOp::Effect(EffectOp::CreateSnippet {
                            text: text.clone(),
                            name: optional_string(name),
                        })),
                    ),
                    _ => Err("create_snippet expects a non-empty text and a name".into()),
                },
            )
            .with_documentation("Opens Create Snippet filled in with the text; the user saves it."),
            MethodDescriptor::new(
                "pick_date",
                vec![
                    ArgumentDescriptor::new("callback", ArgumentSchema::Callback(TEXT_CALLBACK)),
                    ArgumentDescriptor::new(
                        "include_time",
                        ArgumentSchema::Optional(Box::new(ArgumentSchema::Boolean)),
                    ),
                ],
                |arguments| match arguments {
                    [callback @ ComponentArgument::Callback(_), include_time] => Ok(
                        ComponentPayload::new(ActionOp::Effect(EffectOp::PickDate {
                            callback: callback.clone(),
                            include_time: matches!(
                                include_time,
                                ComponentArgument::Optional(Some(value))
                                    if **value == ComponentArgument::Boolean(true)
                            ),
                        })),
                    ),
                    _ => Err("pick_date expects a function and whether to include a time".into()),
                },
            )
            .with_documentation(
                "Asks for a date, then calls back with it as `YYYY-MM-DD`, or \
                 `YYYY-MM-DDTHH:MM` when `include_time` is true; for \"Snooze Until…\".",
            ),
            effect_string("copy", "Copies text to the clipboard.", EffectOp::Copy),
            effect_string(
                "paste",
                "Pastes text into the application that was frontmost before the launcher, \
                 then hides the launcher.",
                EffectOp::Paste,
            ),
            MethodDescriptor::new(
                "toast",
                vec![
                    ArgumentDescriptor::new("title", ArgumentSchema::String),
                    ArgumentDescriptor::new(
                        "style",
                        ArgumentSchema::Optional(Box::new(ArgumentSchema::Enum(TOAST_STYLES))),
                    ),
                    ArgumentDescriptor::new(
                        "message",
                        ArgumentSchema::Optional(Box::new(ArgumentSchema::String)),
                    ),
                ],
                |arguments| match arguments {
                    [ComponentArgument::String(title), style, message] => {
                        let style = match optional_string(style) {
                            None => ToastStyle::Info,
                            Some(style) => parse_toast_style(&style)?,
                        };
                        Ok(ComponentPayload::new(ActionOp::Effect(EffectOp::Toast {
                            title: title.clone(),
                            style,
                            message: optional_string(message),
                        })))
                    }
                    _ => Err("toast expects a title, an optional style and a message".into()),
                },
            )
            .with_documentation(
                "Shows a message in the launcher. `style` is info (the default), success, \
                 failure or progress.",
            ),
            effect_string(
                "hud",
                "Hides the launcher and shows a short message.",
                EffectOp::Hud,
            ),
            effect_callback(
                "run",
                "Calls back into the extension.",
                RUN_CALLBACK,
                EffectOp::Run,
            ),
            effect_callback(
                "submit",
                "Collects the form's values and calls back with them as an object keyed by \
                 field id. Only meaningful in a Form's actions.",
                SUBMIT_CALLBACK,
                EffectOp::Submit,
            ),
            MethodDescriptor::new(
                "push",
                vec![
                    ArgumentDescriptor::new("build", ArgumentSchema::Callback(PUSH_CALLBACK)),
                    ArgumentDescriptor::new(
                        "title",
                        ArgumentSchema::Optional(Box::new(ArgumentSchema::String)),
                    ),
                ],
                |arguments| match arguments {
                    [callback @ ComponentArgument::Callback(_), title] => {
                        Ok(ComponentPayload::new(ActionOp::Effect(EffectOp::Push {
                            callback: callback.clone(),
                            title: optional_string(title),
                        })))
                    }
                    _ => Err("push expects a function returning a View, and a title".into()),
                },
            )
            .with_documentation(
                "Pushes a page: `build` returns an instance of a `View` subclass, whose render \
                 becomes the new page. `title` names it in the footer; it defaults to the \
                 action's title.",
            ),
            effect_string(
                "launch",
                "Opens another command of this extension by name (or `extension-id/command` \
                 for another extension's). Give it arguments with `argument`.",
                EffectOp::Launch,
            ),
            MethodDescriptor::new(
                "argument",
                vec![
                    ArgumentDescriptor::new("name", ArgumentSchema::String),
                    ArgumentDescriptor::new("value", ArgumentSchema::String),
                ],
                |arguments| match arguments {
                    [
                        ComponentArgument::String(name),
                        ComponentArgument::String(value),
                    ] if !name.trim().is_empty() => Ok(ComponentPayload::new(ActionOp::Argument {
                        name: name.clone(),
                        value: value.clone(),
                    })),
                    _ => Err("argument expects a non-empty name and a value".into()),
                },
            )
            .with_documentation(
                "Passes one argument to the command `launch` opens, as `launch().arguments`.",
            ),
            unit_method(
                "pop",
                "Returns to the previous page.",
                ActionOp::Effect(EffectOp::Pop),
            ),
            unit_method(
                "pop_to_root",
                "Returns to the root search.",
                ActionOp::Effect(EffectOp::PopToRoot),
            ),
            unit_method(
                "close_window",
                "Hides the launcher.",
                ActionOp::Effect(EffectOp::CloseWindow),
            ),
            string_method(
                "icon",
                "A Lucide icon name, or a path to an image inside the extension.",
                ActionOp::Icon,
            ),
            string_method(
                "shortcut",
                "A keystroke that performs the action, such as `cmd-shift-c`.",
                ActionOp::Shortcut,
            ),
            unit_method(
                "destructive",
                "Marks the action as deleting or discarding something.",
                ActionOp::Destructive,
            ),
            MethodDescriptor::new(
                "confirm",
                vec![
                    ArgumentDescriptor::new("title", ArgumentSchema::String),
                    ArgumentDescriptor::new(
                        "message",
                        ArgumentSchema::Optional(Box::new(ArgumentSchema::String)),
                    ),
                ],
                |arguments| match arguments {
                    [ComponentArgument::String(title), message] if !title.trim().is_empty() => {
                        Ok(ComponentPayload::new(ActionOp::Confirm {
                            title: title.clone(),
                            message: optional_string(message),
                        }))
                    }
                    _ => Err("confirm expects a non-empty title and an optional message".into()),
                },
            )
            .with_documentation(
                "Asks before performing the effect. A destructive action's confirmation is \
                 drawn as destructive too.",
            ),
        ])
}

pub(crate) fn parse_toast_style(style: &str) -> Result<ToastStyle, String> {
    Ok(match style {
        "info" => ToastStyle::Info,
        "success" => ToastStyle::Success,
        "failure" => ToastStyle::Failure,
        "progress" => ToastStyle::Progress,
        other => {
            return Err(format!(
                "unknown toast style `{other}`; use {}",
                TOAST_STYLES.join(", ")
            ));
        }
    })
}

struct ActionMaterializer;

impl ComponentMaterializer for ActionMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "Action")?;
        let Title(title) = payload(&request, "Action")?;
        let mut icon = None;
        let mut shortcut = None;
        let mut destructive = false;
        let mut confirm = None;
        let mut arguments = Vec::new();
        let mut effect: Option<EffectOp> = None;
        for op in recorded::<ActionOp>(&request) {
            match op {
                ActionOp::Icon(value) => icon = Some(Image::parse(&value)),
                ActionOp::Shortcut(value) => shortcut = Some(value),
                ActionOp::Destructive => destructive = true,
                ActionOp::Confirm { title, message } => confirm = Some((title, message)),
                ActionOp::Argument { name, value } => arguments.push((name, value)),
                ActionOp::Effect(next) => {
                    if let Some(previous) = &effect {
                        bail!(
                            "Action `{title}` has two effects, `{}` and `{}`; give it exactly one",
                            previous.method(),
                            next.method()
                        );
                    }
                    effect = Some(next);
                }
            }
        }
        let effect = effect.with_context(|| {
            format!("Action `{title}` has no effect; call one of {EFFECT_METHODS}")
        })?;
        if !arguments.is_empty() && !matches!(effect, EffectOp::Launch(_)) {
            bail!("Action `{title}` passes arguments, which only `launch` takes");
        }

        let effect = effect_model(effect, &title, arguments, &mut request)?;
        let effect = match confirm {
            Some((confirm_title, message)) => {
                let confirmation =
                    Confirmation::new(confirm_title, effect).destructive(destructive);
                Effect::Confirm(match message {
                    Some(message) => confirmation.with_message(message),
                    None => confirmation,
                })
            }
            None => effect,
        };

        let mut action = Action::new(title, effect);
        if let Some(icon) = icon {
            action = action.with_image(icon);
        }
        if let Some(shortcut) = shortcut {
            action = action.with_shortcut(shortcut);
        }
        if destructive {
            action = action.with_style(ActionStyle::Destructive);
        }
        Ok(carry(action))
    }
}

fn effect_model(
    effect: EffectOp,
    title: &str,
    arguments: Vec<(String, String)>,
    request: &mut MaterializeRequest<'_>,
) -> Result<Effect> {
    Ok(match effect {
        EffectOp::OpenUrl(url) => Effect::OpenUrl(url.into()),
        EffectOp::Open(path) => Effect::OpenPath(path.into()),
        EffectOp::Reveal(path) => Effect::RevealPath(path.into()),
        EffectOp::OpenWith {
            target,
            application,
        } => Effect::OpenWith {
            target: target.into(),
            application: application.into(),
        },
        EffectOp::Trash(paths) => Effect::Trash(paths.into_iter().map(Into::into).collect()),
        EffectOp::QuickLook(path) => Effect::QuickLook(path.into()),
        EffectOp::CreateQuicklink { name, link } => Effect::CreateQuicklink {
            name: name.into(),
            link: link.into(),
        },
        EffectOp::CreateSnippet { text, name } => Effect::CreateSnippet {
            name: name.unwrap_or_default().into(),
            text: text.into(),
        },
        EffectOp::PickDate {
            callback,
            include_time,
        } => {
            let on_pick = text_handler(request.resolve_callback(&callback)?, "Action.pick_date");
            let page_title: SharedString = title.to_owned().into();
            Effect::Push(PushHandler::new(move |_, cx| {
                Ok(crate::pages::pick_date_page(
                    page_title.clone(),
                    include_time,
                    on_pick.clone(),
                    cx,
                ))
            }))
        }
        EffectOp::Copy(text) => Effect::Copy(text.into()),
        EffectOp::Paste(text) => Effect::Paste(text.into()),
        EffectOp::Toast {
            title,
            style,
            message,
        } => {
            let toast = Toast::new(style, title);
            Effect::ShowToast(match message {
                Some(message) => toast.with_message(message),
                None => toast,
            })
        }
        EffectOp::Hud(text) => Effect::ShowHud(text.into()),
        EffectOp::Run(callback) => Effect::Run(run_handler(
            request.resolve_callback(&callback)?,
            "Action.run",
        )),
        EffectOp::Submit(callback) => {
            let callback = request.resolve_callback(&callback)?;
            Effect::SubmitForm(FormHandler::new(move |values, window, cx| {
                if let Err(error) =
                    callback.invoke_data_with(&[form_values_data(&values)], window, cx)
                {
                    tracing::error!("Action.submit: {error:#}");
                }
            }))
        }
        EffectOp::Push {
            callback,
            title: page_title,
        } => {
            let callback = request.resolve_callback(&callback)?;
            let page_title: SharedString = page_title.unwrap_or_else(|| title.to_owned()).into();
            Effect::Push(PushHandler::new(move |window, cx| {
                page_from_callback(&callback, page_title.clone(), window, cx)
            }))
        }
        EffectOp::Launch(command) => {
            let id = launch_target(&command).with_context(|| format!("Action `{title}`"))?;
            Effect::Launch(
                arguments
                    .into_iter()
                    .fold(LaunchRequest::new(id), |request, (name, value)| {
                        request.with_argument(name, value)
                    }),
            )
        }
        EffectOp::Pop => Effect::Pop,
        EffectOp::PopToRoot => Effect::PopToRoot,
        EffectOp::CloseWindow => Effect::CloseWindow,
    })
}

/// `name` is a command of the extension being rendered; `extension/name`
/// names another extension's command explicitly.
fn launch_target(command: &str) -> Result<CommandId> {
    if let Some((extension, name)) = command.rsplit_once('/') {
        return Ok(CommandId::new(extension.to_owned(), name.to_owned()));
    }
    let extension = current_extension().with_context(|| {
        format!(
            "cannot tell which extension `launch(\"{command}\")` belongs to; write \
             `extension-id/{command}`"
        )
    })?;
    Ok(CommandId::new(extension, command.to_owned()))
}
