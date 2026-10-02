//! The Extensions settings: every command in one table, with its alias,
//! hotkey and whether it is enabled, and beside it the settings of the
//! selected command or of its extension.

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        menu::{DropdownMenu as _, PopupMenuItem},
        switch::Switch,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use serde_json::Value;

use super::{
    built_in::{self, Control as BuiltInControl},
    hotkey_recorder::{HotkeyRecorder, SaveHotkey},
    inventory::{self, Command, Feature, Group, Owner},
};
use crate::{
    extensions::{Catalog, PreferenceInput, PreferenceManifest, PreferenceScope, PreferenceStore},
    shell::launcher,
    ui::picture::{Badge, badge},
};

/// What the detail pane shows.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Selection {
    /// A feature or extension, by [`Group::key`].
    Group(String),
    Command {
        group: String,
        id: String,
    },
}

/// A text field's state: one line, or several.
enum TextField {
    Line(Entity<InputState>),
    Area(Entity<TextareaState>),
}

impl TextField {
    fn value(&self, cx: &App) -> SharedString {
        match self {
            Self::Line(input) => input.read(cx).value(),
            Self::Area(input) => input.read(cx).value(),
        }
    }
}

/// The controls of the selected row, made when the selection changes so
/// what is typed survives re-rendering.
#[derive(Default)]
struct Detail {
    alias: Option<Entity<InputState>>,
    hotkey: Option<Entity<HotkeyRecorder>>,
    /// Text fields by preference key.
    texts: HashMap<String, TextField>,
    /// Extension preference values, read once per selection.
    values: HashMap<String, Value>,
    /// Errors by field key; `alias` for the alias.
    errors: HashMap<String, SharedString>,
    _subscriptions: Vec<Subscription>,
}

pub struct ExtensionsPane {
    groups: Vec<Group>,
    store: Option<PreferenceStore>,
    /// Extension preference declarations by extension id, then by command
    /// name (`None` for the extension's own).
    declarations: HashMap<SharedString, HashMap<Option<String>, Vec<PreferenceManifest>>>,
    search: Entity<InputState>,
    expanded: HashSet<String>,
    selected: Option<Selection>,
    detail: Detail,
    scroll: ScrollHandle,
    /// Set by [`Self::reveal`]: the table scrolls to the selection once
    /// it is drawn.
    scroll_to_selection: bool,
    _subscriptions: Vec<Subscription>,
}

/// The height of every row of the table.
const ROW_HEIGHT: gpui_kit::Pixels = px(34.);

/// The key of an extension preference's field.
fn preference_key(scope: &PreferenceScope, name: &str) -> String {
    format!("{}#{name}", scope.key())
}

impl ExtensionsPane {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search commands…"));
        let mut subscriptions = vec![cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })];
        if let Some(store) = crate::customizations::store(cx) {
            subscriptions.push(cx.observe(&store, |_, _, cx| cx.notify()));
        }
        let mut pane = Self {
            groups: Vec::new(),
            store: None,
            declarations: HashMap::new(),
            search,
            expanded: HashSet::new(),
            selected: None,
            detail: Detail::default(),
            scroll: ScrollHandle::new(),
            scroll_to_selection: false,
            _subscriptions: subscriptions,
        };
        pane.reload(cx);
        if let Some(group) = pane.groups.first() {
            let key = group.key();
            pane.select(Selection::Group(key), window, cx);
        }
        pane
    }

    /// Reads the commands and extensions again, as after one is installed.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some((catalog, host)) = launcher::host_and_catalog(cx) else {
            return;
        };
        self.groups = inventory::groups(&catalog);
        self.store = Some(host.preference_store());
        self.declarations = declarations(&catalog);
        cx.notify();
    }

    /// Selects the group of the extension `id`, or the command `id`, and
    /// shows it.
    pub fn reveal(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let found = self.groups.iter().find_map(|group| {
            let matches_group = group.key() == id
                || matches!(&group.owner, Owner::Extension { id: extension, .. } if extension.as_ref() == id);
            match (matches_group, group.command(id)) {
                (true, _) => Some(Selection::Group(group.key())),
                (false, Some(command)) => Some(Selection::Command {
                    group: group.key(),
                    id: command.id.clone(),
                }),
                (false, None) => None,
            }
        });
        if let Some(selection) = found {
            if let Selection::Command { group, .. } = &selection {
                self.expanded.insert(group.clone());
            }
            self.select(selection, window, cx);
            self.scroll_to_selection = true;
        }
    }

    fn group(&self, key: &str) -> Option<&Group> {
        self.groups.iter().find(|group| group.key() == key)
    }

    // MARK: Selection

    fn select(&mut self, selection: Selection, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&selection) {
            return;
        }
        self.detail = Detail::default();
        match &selection {
            Selection::Group(key) => {
                if let Some(group) = self.group(key).cloned() {
                    self.prepare_group(&group, window, cx);
                }
            }
            Selection::Command { group, id } => {
                if let Some(group) = self.group(group).cloned() {
                    self.prepare_command(&group, id, window, cx);
                }
            }
        }
        self.selected = Some(selection);
        cx.notify();
    }

    fn prepare_group(&mut self, group: &Group, window: &mut Window, cx: &mut Context<Self>) {
        match &group.owner {
            Owner::BuiltIn(feature) => {
                let settings = launcher::settings(cx);
                for preference in built_in::preferences(*feature) {
                    if let BuiltInControl::Text {
                        placeholder,
                        multiline,
                        value,
                        ..
                    } = &preference.control
                    {
                        let text = value(&settings);
                        self.add_text_field(
                            preference.id.to_owned(),
                            text,
                            placeholder.clone(),
                            *multiline,
                            false,
                            window,
                            cx,
                        );
                    }
                }
            }
            Owner::Extension { id, .. } => {
                self.prepare_extension_preferences(
                    &PreferenceScope::extension(id.to_string()),
                    id,
                    None,
                    window,
                    cx,
                );
            }
        }
    }

    fn prepare_command(
        &mut self,
        group: &Group,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (alias, hotkey) = crate::customizations::store(cx)
            .map(|store| {
                let store = store.read(cx);
                (
                    store.alias(id).unwrap_or_default().to_owned(),
                    store.hotkey(id).map(str::to_owned),
                )
            })
            .unwrap_or_default();

        let alias_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Add Alias")
                .default_value(alias)
        });
        let item = id.to_owned();
        self.detail._subscriptions.push(cx.subscribe(
            &alias_input,
            move |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    let alias = input.read(cx).value().trim().to_owned();
                    match save_alias(&item, &alias, cx) {
                        Ok(()) => this.detail.errors.remove("alias"),
                        Err(error) => this.detail.errors.insert("alias".into(), error.into()),
                    };
                    cx.notify();
                }
            },
        ));
        self.detail.alias = Some(alias_input);

        let item = id.to_owned();
        let save: SaveHotkey = Rc::new(move |shortcut, _, cx| save_hotkey(&item, shortcut, cx));
        self.detail.hotkey = Some(cx.new(|cx| HotkeyRecorder::new(hotkey, save, cx)));

        if let Owner::Extension { id: extension, .. } = &group.owner
            && let Some(name) = id.rsplit('/').next()
        {
            self.prepare_extension_preferences(
                &PreferenceScope::command(extension.to_string(), name.to_owned()),
                extension,
                Some(name.to_owned()),
                window,
                cx,
            );
        }
    }

    fn prepare_extension_preferences(
        &mut self,
        scope: &PreferenceScope,
        extension: &SharedString,
        command: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let declarations = self
            .declarations
            .get(extension)
            .and_then(|by_command| by_command.get(&command))
            .cloned()
            .unwrap_or_default();
        for declaration in declarations {
            let key = preference_key(scope, &declaration.name);
            let value = store
                .value(scope, &declaration)
                .map_err(|error| tracing::warn!("cannot read a preference: {error:#}"))
                .ok()
                .flatten();
            match declaration.input {
                PreferenceInput::Text | PreferenceInput::Password => {
                    let text = value
                        .as_ref()
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    self.add_text_field(
                        key.clone(),
                        text,
                        SharedString::default(),
                        false,
                        declaration.input == PreferenceInput::Password,
                        window,
                        cx,
                    );
                }
                PreferenceInput::Checkbox | PreferenceInput::Dropdown => {}
            }
            if let Some(value) = value {
                self.detail.values.insert(key, value);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_text_field(
        &mut self,
        key: String,
        text: String,
        placeholder: SharedString,
        multiline: bool,
        masked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let field = match multiline {
            true => TextField::Area(cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(3, 6)
                    .placeholder(placeholder)
                    .default_value(text)
            })),
            false => TextField::Line(cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(masked)
                    .placeholder(placeholder)
                    .default_value(text)
            })),
        };
        let on_event = {
            let key = key.clone();
            move |this: &mut Self, event: &InputEvent, cx: &mut Context<Self>| {
                let commit = match event {
                    InputEvent::Blur => true,
                    InputEvent::PressEnter { .. } => !multiline,
                    InputEvent::Change | InputEvent::Focus => false,
                };
                if commit {
                    this.commit_text(&key, cx);
                }
            }
        };
        let subscription = match &field {
            TextField::Line(input) => {
                cx.subscribe(input, move |this, _, event, cx| on_event(this, event, cx))
            }
            TextField::Area(input) => {
                cx.subscribe(input, move |this, _, event, cx| on_event(this, event, cx))
            }
        };
        self.detail._subscriptions.push(subscription);
        self.detail.texts.insert(key, field);
    }

    /// Saves the text field `key` of the selection.
    fn commit_text(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(text) = self.detail.texts.get(key).map(|field| field.value(cx)) else {
            return;
        };
        let result = match self.selected_group().map(|group| group.owner.clone()) {
            Some(Owner::BuiltIn(feature)) => built_in::preferences(feature)
                .into_iter()
                .find(|preference| preference.id == key)
                .map_or(Ok(()), |preference| match preference.control {
                    BuiltInControl::Text { set, .. } => set(launcher::settings(cx), &text)
                        .and_then(|settings| save_settings(settings, cx)),
                    BuiltInControl::Switch { .. } | BuiltInControl::Dropdown { .. } => Ok(()),
                }),
            Some(Owner::Extension { .. }) => {
                let value =
                    (!text.trim().is_empty()).then(|| Value::String(text.trim().to_owned()));
                self.set_extension_preference(key, value, cx)
            }
            None => Ok(()),
        };
        match result {
            Ok(()) => self.detail.errors.remove(key),
            Err(error) => self.detail.errors.insert(key.to_owned(), error.into()),
        };
        cx.notify();
    }

    /// Stores an extension preference of the selection by its field key.
    fn set_extension_preference(
        &mut self,
        key: &str,
        value: Option<Value>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let (scope, declaration) = self
            .extension_declarations()
            .into_iter()
            .find(|(scope, declaration)| preference_key(scope, &declaration.name) == key)
            .ok_or_else(|| "This setting no longer exists.".to_owned())?;
        let store = self
            .store
            .clone()
            .ok_or_else(|| "Extensions are not loaded.".to_owned())?;
        store
            .set(&scope, &declaration, value.clone())
            .map_err(|error| format!("{error:#}"))?;
        match value {
            Some(value) => self.detail.values.insert(key.to_owned(), value),
            None => self.detail.values.remove(key),
        };
        cx.notify();
        Ok(())
    }

    fn selected_group(&self) -> Option<&Group> {
        match self.selected.as_ref()? {
            Selection::Group(key) | Selection::Command { group: key, .. } => self.group(key),
        }
    }

    /// The extension preferences the selection shows, with their scopes.
    fn extension_declarations(&self) -> Vec<(PreferenceScope, PreferenceManifest)> {
        let Some(Owner::Extension { id, .. }) = self.selected_group().map(|group| &group.owner)
        else {
            return Vec::new();
        };
        let (scope, command) = match &self.selected {
            Some(Selection::Command { id: command, .. }) => {
                let name = command.rsplit('/').next().unwrap_or_default().to_owned();
                (
                    PreferenceScope::command(id.to_string(), name.clone()),
                    Some(name),
                )
            }
            _ => (PreferenceScope::extension(id.to_string()), None),
        };
        self.declarations
            .get(id)
            .and_then(|by_command| by_command.get(&command))
            .into_iter()
            .flatten()
            .map(|declaration| (scope.clone(), declaration.clone()))
            .collect()
    }

    fn toggle_expanded(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        cx.notify();
    }

    // MARK: Drawing

    fn render_table(&mut self, query: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = &cx.theme().clone();
        let disabled = crate::customizations::store(cx)
            .map(|store| store.read(cx).disabled().clone())
            .unwrap_or_default();
        let (aliases, hotkeys): (HashMap<String, String>, HashMap<String, String>) =
            crate::customizations::store(cx)
                .map(|store| {
                    let store = store.read(cx);
                    (
                        store
                            .aliases()
                            .map(|(item, alias)| (item.to_owned(), alias.to_owned()))
                            .collect(),
                        store
                            .hotkeys()
                            .map(|(item, hotkey)| (item.to_owned(), hotkey.to_owned()))
                            .collect(),
                    )
                })
                .unwrap_or_default();
        let query = query.to_lowercase();
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut selected_row = None;
        for group in &self.groups {
            let group_matches = query.is_empty() || group.title.to_lowercase().contains(&query);
            let commands: Vec<&Command> = group
                .commands
                .iter()
                .filter(|command| group_matches || command.title.to_lowercase().contains(&query))
                .collect();
            if !group_matches && commands.is_empty() {
                continue;
            }
            let key = group.key();
            let expanded = !query.is_empty() || self.expanded.contains(&key);
            let enabled_count = group
                .commands
                .iter()
                .filter(|command| !disabled.contains(&command.id))
                .count();
            if self.selected == Some(Selection::Group(key.clone())) {
                selected_row = Some(rows.len());
            }
            rows.push(self.group_row(group, expanded, enabled_count, cx));
            if expanded {
                for command in commands {
                    if matches!(&self.selected, Some(Selection::Command { id, .. }) if *id == command.id)
                    {
                        selected_row = Some(rows.len());
                    }
                    rows.push(self.command_row(
                        &key,
                        command,
                        !disabled.contains(&command.id),
                        aliases.get(&command.id).cloned(),
                        hotkeys.get(&command.id).cloned(),
                        cx,
                    ));
                }
            }
        }
        if let Some(row) = selected_row.filter(|_| self.scroll_to_selection) {
            self.scroll_to_selection = false;
            // Rows are one height, so the offset is known before layout,
            // which a newly opened window has not done yet.
            let top = (ROW_HEIGHT * row as f32 - ROW_HEIGHT * 3.).max(px(0.));
            self.scroll.set_offset(gpui_kit::point(px(0.), -top));
        }
        let header = h_flex()
            .flex_none()
            .h(px(28.))
            .px_2()
            .gap_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .border_b_1()
            .border_color(theme.border)
            .child(div().flex_1().child("Name"))
            .child(div().w(px(76.)).child("Type"))
            .child(div().w(px(64.)).child("Alias"))
            .child(div().w(px(104.)).child("Hotkey"))
            .child(div().w(px(52.)).text_right().child("Enabled"));
        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(header)
            .child(
                div()
                    .id("extension-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows),
            )
            .into_any_element()
    }

    fn row_frame(
        &self,
        id: SharedString,
        selected: bool,
        cx: &App,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let theme = &cx.theme().clone();
        h_flex()
            .id(id)
            .h(ROW_HEIGHT)
            .px_2()
            .gap_2()
            .rounded(theme.radius)
            .text_sm()
            .cursor_default()
            .when(selected, |this| this.bg(theme.accent))
            .when(!selected, |this| {
                this.hover(|this| this.bg(theme.foreground.opacity(0.04)))
            })
    }

    fn group_row(
        &self,
        group: &Group,
        expanded: bool,
        enabled_count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &cx.theme().clone();
        let key = group.key();
        let selected = self.selected == Some(Selection::Group(key.clone()));
        let total = group.commands.len();
        let ids: Vec<String> = group
            .commands
            .iter()
            .map(|command| command.id.clone())
            .collect();
        let kind = match &group.owner {
            Owner::BuiltIn(_) => "Built-in",
            Owner::Extension { .. } => "Extension",
        };
        self.row_frame(format!("group-{key}").into(), selected, cx)
            .on_click(cx.listener({
                let key = key.clone();
                move |this, _, window, cx| this.select(Selection::Group(key.clone()), window, cx)
            }))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .child(
                        Button::new(SharedString::from(format!("expand-{key}")))
                            .ghost()
                            .xsmall()
                            .icon(match expanded {
                                true => IconName::ChevronDown,
                                false => IconName::ChevronRight,
                            })
                            .disabled(total == 0)
                            .tab_stop(false)
                            .on_click(cx.listener({
                                let key = key.clone();
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_expanded(key.clone(), cx)
                                }
                            })),
                    )
                    .child(badge(
                        &group.image,
                        Badge::Colored,
                        theme.muted_foreground,
                        theme,
                    ))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(group.title.clone()),
                    ),
            )
            .child(
                div()
                    .w(px(76.))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(kind),
            )
            .child(div().w(px(64.)))
            .child(div().w(px(104.)))
            .child(h_flex().w(px(52.)).justify_end().when(total > 0, |this| {
                this.child(
                    Checkbox::new(SharedString::from(format!("enable-{key}")))
                        .checked(enabled_count > 0)
                        .on_click(move |checked, _, cx| {
                            set_enabled(&ids, *checked, cx);
                        }),
                )
            }))
            .into_any_element()
    }

    fn command_row(
        &self,
        group: &str,
        command: &Command,
        enabled: bool,
        alias: Option<String>,
        hotkey: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &cx.theme().clone();
        let selection = Selection::Command {
            group: group.to_owned(),
            id: command.id.clone(),
        };
        let selected = self.selected.as_ref() == Some(&selection);
        let id = command.id.clone();
        self.row_frame(format!("command-{}", command.id).into(), selected, cx)
            .on_click(
                cx.listener(move |this, _, window, cx| this.select(selection.clone(), window, cx)),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .pl(px(30.))
                    .when(!enabled, |this| this.opacity(0.5))
                    .child(div().size(px(22.)).flex_none().when_some(
                        command.image.as_ref(),
                        |this, image| {
                            this.child(badge(image, Badge::Colored, theme.muted_foreground, theme))
                        },
                    ))
                    .child(div().min_w_0().truncate().child(command.title.clone())),
            )
            .child(
                div()
                    .w(px(76.))
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(command.kind.clone()),
            )
            .child(
                div()
                    .w(px(64.))
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .when_some(alias, |this, alias| this.child(alias)),
            )
            .child(
                div()
                    .w(px(104.))
                    .overflow_hidden()
                    .when_some(hotkey, |this, hotkey| {
                        this.child(crate::ui::keycaps::hotkey_caps(&hotkey, cx))
                    }),
            )
            .child(
                h_flex().w(px(52.)).justify_end().child(
                    Checkbox::new(SharedString::from(format!("enable-{id}")))
                        .checked(enabled)
                        .on_click(move |checked, _, cx| {
                            set_enabled(std::slice::from_ref(&id), *checked, cx)
                        }),
                ),
            )
            .into_any_element()
    }

    fn render_detail(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(group) = self.selected_group().cloned() else {
            return div().into_any_element();
        };
        let command = match &self.selected {
            Some(Selection::Command { id, .. }) => group.command(id).cloned(),
            _ => None,
        };
        let theme = &cx.theme().clone();
        let (image, title, subtitle) = match &command {
            Some(command) => (
                command.image.clone().unwrap_or_else(|| group.image.clone()),
                command.title.clone(),
                Some(group.title.clone()),
            ),
            None => (group.image.clone(), group.title.clone(), None),
        };
        let description: Option<SharedString> = match (&command, &group.owner) {
            (Some(_), _) => None,
            (None, Owner::BuiltIn(feature)) => Some(feature.description().into()),
            (None, Owner::Extension { description, .. }) => description.clone(),
        };
        let byline: Option<SharedString> = match (&command, &group.owner) {
            (
                None,
                Owner::Extension {
                    author, version, ..
                },
            ) => Some(
                match author {
                    Some(author) => format!("{author} · Version {version}"),
                    None => format!("Version {version}"),
                }
                .into(),
            ),
            _ => None,
        };

        let header = h_flex()
            .gap_3()
            .child(div().flex_none().child(badge(
                &image,
                Badge::Colored,
                theme.muted_foreground,
                theme,
            )))
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .truncate()
                            .child(title),
                    )
                    .when_some(subtitle, |this, subtitle| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(subtitle),
                        )
                    }),
            );

        let mut sections: Vec<AnyElement> = vec![header.into_any_element()];
        if let Some(description) = description {
            sections.push(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(description)
                    .into_any_element(),
            );
        }
        if let Some(byline) = byline {
            sections.push(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(byline)
                    .into_any_element(),
            );
        }

        if let Some(command) = &command {
            sections.push(self.command_fields(command, cx));
        }
        let preferences = match (&command, &group.owner) {
            (None, Owner::BuiltIn(feature)) => self.built_in_fields(*feature, window, cx),
            (_, Owner::Extension { .. }) => self.extension_fields(cx),
            (Some(_), Owner::BuiltIn(_)) => Vec::new(),
        };
        if !preferences.is_empty() {
            sections.push(
                v_flex()
                    .gap_3()
                    .pt_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(theme.muted_foreground)
                            .child("Settings"),
                    )
                    .children(preferences)
                    .into_any_element(),
            );
        }
        if let Some(command) = command {
            let id = command.id.clone();
            sections.push(
                h_flex()
                    .child(
                        Button::new("open-command")
                            .small()
                            .outline()
                            .label("Open Command")
                            .on_click(move |_, _, cx| launcher::open_item(id.clone(), cx)),
                    )
                    .into_any_element(),
            );
        }
        v_flex()
            .id("extension-detail")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .gap_4()
            .children(sections)
            .into_any_element()
    }

    fn command_fields(&self, command: &Command, cx: &mut Context<Self>) -> AnyElement {
        let theme = &cx.theme().clone();
        let enabled = crate::customizations::store(cx)
            .is_none_or(|store| !store.read(cx).is_disabled(&command.id));
        let id = command.id.clone();
        v_flex()
            .gap_3()
            .child(field_row(
                "Enabled",
                None,
                Switch::new("command-enabled")
                    .checked(enabled)
                    .on_click(move |checked, _, cx| {
                        set_enabled(std::slice::from_ref(&id), *checked, cx)
                    })
                    .into_any_element(),
                theme,
            ))
            .when_some(self.detail.alias.as_ref(), |this, alias| {
                this.child(field_row(
                    "Alias",
                    self.detail.errors.get("alias").cloned(),
                    div()
                        .w(px(160.))
                        .child(Input::new(alias).small())
                        .into_any_element(),
                    theme,
                ))
            })
            .when_some(self.detail.hotkey.as_ref(), |this, hotkey| {
                this.child(field_row(
                    "Hotkey",
                    None,
                    hotkey.clone().into_any_element(),
                    theme,
                ))
            })
            .into_any_element()
    }

    fn built_in_fields(
        &self,
        feature: Feature,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let settings = launcher::settings(cx);
        let theme = &cx.theme().clone();
        built_in::preferences(feature)
            .into_iter()
            .map(|preference| {
                // A text field takes the panel's width, under its title.
                let stacked = matches!(preference.control, BuiltInControl::Text { .. });
                let control = match preference.control {
                    BuiltInControl::Switch { value, set } => Switch::new(preference.id)
                        .checked(value(&settings))
                        .on_click(move |checked, _, cx| {
                            let settings = set(launcher::settings(cx), *checked);
                            save_settings(settings, cx).ok();
                        })
                        .into_any_element(),
                    BuiltInControl::Dropdown {
                        choices,
                        value,
                        set,
                    } => {
                        let current = value(&settings);
                        let title = choices
                            .iter()
                            .find(|(value, _)| *value == current)
                            .map(|(_, title)| title.clone())
                            .unwrap_or(current.clone());
                        dropdown_button(preference.id, title, choices, current, move |value, cx| {
                            let settings = set(launcher::settings(cx), value);
                            save_settings(settings, cx).ok();
                        })
                    }
                    BuiltInControl::Text { multiline, .. } => {
                        self.text_control(preference.id, multiline)
                    }
                };
                field_block(
                    preference.title.clone(),
                    preference.description.clone(),
                    self.detail.errors.get(preference.id).cloned(),
                    control,
                    stacked,
                    theme,
                )
            })
            .collect()
    }

    fn text_control(&self, key: &str, multiline: bool) -> AnyElement {
        match self.detail.texts.get(key) {
            Some(TextField::Line(input)) => div()
                .w_full()
                .child(Input::new(input).small())
                .into_any_element(),
            Some(TextField::Area(input)) => div()
                .w_full()
                .child(Textarea::new(input))
                .into_any_element(),
            None if multiline => div().into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn extension_fields(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = &cx.theme().clone();
        let pane = cx.entity().downgrade();
        self.extension_declarations()
            .into_iter()
            .map(|(scope, declaration)| {
                let key = preference_key(&scope, &declaration.name);
                let value = self.detail.values.get(&key).cloned();
                let control = match declaration.input {
                    PreferenceInput::Text | PreferenceInput::Password => {
                        self.text_control(&key, false)
                    }
                    PreferenceInput::Checkbox => {
                        let pane = pane.clone();
                        let key = key.clone();
                        Checkbox::new(SharedString::from(key.clone()))
                            .when_some(declaration.label.clone(), |this, label| this.label(label))
                            .checked(value.as_ref().and_then(Value::as_bool).unwrap_or(false))
                            .on_click(move |checked, _, cx| {
                                let checked = *checked;
                                pane.update(cx, |pane, cx| {
                                    if let Err(error) = pane.set_extension_preference(
                                        &key,
                                        Some(Value::Bool(checked)),
                                        cx,
                                    ) {
                                        pane.detail.errors.insert(key.clone(), error.into());
                                    }
                                })
                                .ok();
                            })
                            .into_any_element()
                    }
                    PreferenceInput::Dropdown => {
                        let current: SharedString = value
                            .as_ref()
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned()
                            .into();
                        let choices: Vec<(SharedString, SharedString)> = declaration
                            .choices
                            .iter()
                            .map(|choice| {
                                (choice.value.clone().into(), choice.title.clone().into())
                            })
                            .collect();
                        let title = choices
                            .iter()
                            .find(|(value, _)| *value == current)
                            .map(|(_, title)| title.clone())
                            .unwrap_or_else(|| "Choose…".into());
                        let pane = pane.clone();
                        let key_for_set = key.clone();
                        dropdown_button(
                            SharedString::from(key.clone()),
                            title,
                            choices,
                            current,
                            move |value, cx| {
                                let value = value.to_owned();
                                pane.update(cx, |pane, cx| {
                                    pane.set_extension_preference(
                                        &key_for_set,
                                        Some(Value::String(value)),
                                        cx,
                                    )
                                    .ok();
                                })
                                .ok();
                            },
                        )
                    }
                };
                let title: SharedString = match declaration.required {
                    true => format!("{} (required)", declaration.title).into(),
                    false => declaration.title.clone().into(),
                };
                field_block(
                    title,
                    declaration.description.clone().map(Into::into),
                    self.detail.errors.get(&key).cloned(),
                    control,
                    matches!(
                        declaration.input,
                        PreferenceInput::Text | PreferenceInput::Password
                    ),
                    theme,
                )
            })
            .collect()
    }
}

impl Render for ExtensionsPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).value().to_string();
        let table = self.render_table(query.trim(), cx);
        let detail = self.render_detail(window, cx);
        let theme = &cx.theme().clone();
        h_flex()
            .size_full()
            .items_stretch()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap_2()
                    .pr_3()
                    .child(
                        Input::new(&self.search)
                            .small()
                            .prefix(Icon::new(IconName::Search).small()),
                    )
                    .child(v_flex().flex_1().min_h_0().child(table)),
            )
            .child(
                div()
                    .w(px(300.))
                    .flex_none()
                    .h_full()
                    .border_l_1()
                    .border_color(theme.border)
                    .child(detail),
            )
    }
}

// MARK: Helpers

/// A label and its control on one line, with an error underneath.
fn field_row(
    title: &str,
    error: Option<SharedString>,
    control: AnyElement,
    theme: &gpui_kit::component::Theme,
) -> AnyElement {
    field_block(title.to_owned().into(), None, error, control, false, theme)
}

/// A setting: its title and description, and its control beside them or,
/// when `stacked`, under them.
fn field_block(
    title: SharedString,
    description: Option<SharedString>,
    error: Option<SharedString>,
    control: AnyElement,
    stacked: bool,
    theme: &gpui_kit::component::Theme,
) -> AnyElement {
    let label = v_flex()
        .min_w_0()
        .flex_1()
        .gap_0p5()
        .child(div().text_sm().child(title))
        .when_some(description, |this, description| {
            this.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(description),
            )
        });
    v_flex()
        .gap_1()
        .map(|this| match stacked {
            true => this.child(label).child(control),
            false => this.child(
                h_flex()
                    .gap_3()
                    .justify_between()
                    .child(label)
                    .child(control),
            ),
        })
        .when_some(error, |this, error| {
            this.child(div().text_xs().text_color(theme.danger).child(error))
        })
        .into_any_element()
}

/// A button that opens a menu of `choices`, checking the current one.
fn dropdown_button(
    id: impl Into<SharedString>,
    title: SharedString,
    choices: Vec<(SharedString, SharedString)>,
    current: SharedString,
    on_choose: impl Fn(&str, &mut App) + 'static,
) -> AnyElement {
    let on_choose = Rc::new(on_choose);
    Button::new(id.into())
        .small()
        .outline()
        .label(title)
        .dropdown_caret(true)
        .dropdown_menu(move |menu, _, _| {
            choices.iter().fold(menu, |menu, (value, title)| {
                let on_choose = on_choose.clone();
                let value = value.clone();
                menu.item(
                    PopupMenuItem::new(title.clone())
                        .checked(value == current)
                        .on_click(move |_, _, cx| on_choose(&value, cx)),
                )
            })
        })
        .into_any_element()
}

fn declarations(
    catalog: &Catalog,
) -> HashMap<SharedString, HashMap<Option<String>, Vec<PreferenceManifest>>> {
    let mut all: HashMap<SharedString, HashMap<Option<String>, Vec<PreferenceManifest>>> =
        HashMap::new();
    for (extension, command) in catalog.commands() {
        let by_command = all.entry(extension.id().clone()).or_default();
        by_command
            .entry(None)
            .or_insert_with(|| extension.preferences().to_vec());
        by_command.insert(
            Some(command.id().command().to_string()),
            command.preferences().to_vec(),
        );
    }
    all
}

fn save_settings(settings: crate::shell::settings::Settings, cx: &mut App) -> Result<(), String> {
    launcher::update_settings(settings, None, cx).map_err(|error| format!("{error:#}"))
}

fn set_enabled(ids: &[String], enabled: bool, cx: &mut App) {
    if let Some(store) = crate::customizations::store(cx) {
        store.update(cx, |store, cx| {
            store.set_enabled(ids.iter().map(String::as_str), enabled, cx)
        });
    }
}

fn save_alias(item: &str, alias: &str, cx: &mut App) -> Result<(), String> {
    if alias.contains(char::is_whitespace) {
        return Err("An alias is one word.".into());
    }
    let Some(store) = crate::customizations::store(cx) else {
        return Ok(());
    };
    if let Some(other) = store.read(cx).item_with_alias(alias)
        && other != item
        && !alias.is_empty()
    {
        return Err("Another command already uses this alias.".into());
    }
    if store.read(cx).alias(item).unwrap_or_default() != alias {
        store.update(cx, |store, cx| store.set_alias(item, alias, cx));
    }
    Ok(())
}

/// Saves a command's hotkey after checking it is not already taken.
fn save_hotkey(item: &str, shortcut: Option<String>, cx: &mut App) -> Result<(), String> {
    let shortcut = shortcut.unwrap_or_default();
    if !shortcut.is_empty() {
        crate::shell::hotkey::parse_shortcut(&shortcut).map_err(|error| error.to_string())?;
        if shortcut.eq_ignore_ascii_case(launcher::settings(cx).summon_shortcut()) {
            return Err("This hotkey opens the launcher.".into());
        }
        let taken = crate::customizations::store(cx).and_then(|store| {
            store
                .read(cx)
                .hotkeys()
                .find(|(other, existing)| {
                    *other != item && existing.eq_ignore_ascii_case(&shortcut)
                })
                .map(|(other, _)| other.to_owned())
        });
        if let Some(other) = taken {
            return Err(format!("Already used by {}.", title_of(&other, cx)));
        }
    }
    launcher::set_command_hotkey(item, &shortcut, cx).map_err(|error| format!("{error:#}"))
}

/// The title of the command `id`, for messages.
fn title_of(id: &str, cx: &App) -> String {
    launcher::host_and_catalog(cx)
        .and_then(|(catalog, _)| {
            inventory::groups(&catalog)
                .into_iter()
                .flat_map(|group| group.commands)
                .find(|command| command.id == id)
                .map(|command| command.title.to_string())
        })
        .unwrap_or_else(|| id.to_owned())
}
