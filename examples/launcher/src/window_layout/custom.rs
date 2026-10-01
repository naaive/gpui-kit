//! Custom layouts: a user's own place for windows, as a position and size
//! in percent of the display, each one a Window Management command.

use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Window};
use serde::{Deserialize, Serialize};

use super::{CustomFrame, Layout};
use crate::{
    model::{
        Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Confirmation, Control,
        Effect, Field, FormHandler, FormModel, FormValue, FormValues, Image, Item, PageModel,
        RunHandler, Toast, ToastStyle,
    },
    pages::{self, Page, PageHandle},
    shell::launcher::perform,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CustomLayout {
    pub id: String,
    pub name: String,
    pub frame: CustomFrame,
}

fn path() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("window-layouts.json"))
}

pub fn load() -> Vec<CustomLayout> {
    path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(layouts: &[CustomLayout]) -> Result<()> {
    let path = path().ok_or_else(|| anyhow::anyhow!("no data directory"))?;
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(layouts)?)?;
    Ok(())
}

/// A custom layout as a command, with the actions to edit or delete it.
pub fn item(layout: &CustomLayout) -> Item {
    let frame = layout.frame;
    let percent = |value: u16| f32::from(value) / 10.;
    let edit = layout.clone();
    let delete = layout.id.clone();
    super::Layout::Custom(frame)
        .item_with(format!("window/custom-{}", layout.id), layout.name.clone())
        .with_subtitle(format!(
            "Custom Layout · {}%, {}% · {}% × {}%",
            percent(frame.x),
            percent(frame.y),
            percent(frame.width),
            percent(frame.height)
        ))
        .with_actions(
            ActionPanel::new()
                .with_action(Action::new(
                    layout.name.clone(),
                    Effect::Run(RunHandler::new(move |(), _, cx| {
                        super::run(Layout::Custom(frame), cx)
                    })),
                ))
                .with_action(
                    Action::new(
                        "Edit Layout",
                        Effect::Push(crate::model::PushHandler::new(move |window, cx| {
                            layout_form(Some(edit.clone()), window, cx)
                        })),
                    )
                    .with_image(Image::Icon("pencil".into()))
                    .with_shortcut("secondary-e"),
                )
                .with_section(
                    ActionSection::new().with_entry(ActionEntry::Action(
                        Action::new(
                            "Delete Layout",
                            Effect::Confirm(
                                Confirmation::new(
                                    "Delete this layout?",
                                    Effect::Run(RunHandler::new(move |(), _, cx| {
                                        let mut layouts = load();
                                        layouts.retain(|known| known.id != delete);
                                        if save(&layouts).is_ok() {
                                            perform(Effect::ShowHud("Layout deleted".into()), cx);
                                        }
                                    })),
                                )
                                .with_confirm_title("Delete")
                                .destructive(true),
                            ),
                        )
                        .with_image(Image::Icon("trash".into()))
                        .with_style(ActionStyle::Destructive)
                        .with_shortcut("ctrl-x"),
                    )),
                ),
        )
}

const NAME: &str = "name";
const FIELDS: [(&str, &str, &str); 4] = [
    ("x", "Left (%)", "0"),
    ("y", "Top (%)", "0"),
    ("width", "Width (%)", "50"),
    ("height", "Height (%)", "100"),
];

/// Creates a custom layout, or edits `editing`.
pub fn layout_form(
    editing: Option<CustomLayout>,
    _: &mut Window,
    cx: &mut App,
) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|_| LayoutForm {
        editing,
        draft: None,
        errors: HashMap::new(),
    })))
}

struct LayoutForm {
    editing: Option<CustomLayout>,
    draft: Option<FormValues>,
    errors: HashMap<&'static str, &'static str>,
}

fn text(values: &FormValues, id: &str) -> String {
    match values.get(id) {
        Some(FormValue::Text(text)) => text.trim().to_owned(),
        _ => String::new(),
    }
}

impl LayoutForm {
    fn submit(&mut self, values: FormValues, cx: &mut Context<Self>) {
        self.errors.clear();
        let name = text(&values, NAME);
        if name.is_empty() {
            self.errors.insert(NAME, "Give the layout a name");
        }
        let mut parts = [0u16; 4];
        for (index, (id, _, _)) in FIELDS.iter().enumerate() {
            match text(&values, id).trim_end_matches('%').parse::<f32>() {
                Ok(value) if (0. ..=100.).contains(&value) => {
                    parts[index] = (value * 10.).round() as u16
                }
                _ => {
                    self.errors.insert(id, "A percentage from 0 to 100");
                }
            }
        }
        if parts[2] == 0 || parts[3] == 0 {
            self.errors.insert("width", "The window needs a size");
        }
        // The window has to fit on the display.
        if parts[0] + parts[2] > 1000 {
            self.errors
                .insert("width", "Left and width add up to more than 100%");
        }
        if parts[1] + parts[3] > 1000 {
            self.errors
                .insert("height", "Top and height add up to more than 100%");
        }
        if !self.errors.is_empty() {
            self.draft = Some(values);
            cx.notify();
            return;
        }
        let layout = CustomLayout {
            id: self
                .editing
                .as_ref()
                .map(|editing| editing.id.clone())
                .unwrap_or_else(|| chrono::Local::now().format("%Y%m%d%H%M%S%3f").to_string()),
            name: name.clone(),
            frame: CustomFrame {
                x: parts[0],
                y: parts[1],
                width: parts[2],
                height: parts[3],
            },
        };
        let mut layouts = load();
        match layouts.iter_mut().find(|known| known.id == layout.id) {
            Some(known) => *known = layout,
            None => layouts.push(layout),
        }
        let toast = match save(&layouts) {
            Ok(()) => Toast::new(ToastStyle::Success, format!("Saved “{name}”")),
            Err(error) => Toast::new(ToastStyle::Failure, "Couldn’t save the layout")
                .with_message(format!("{error:#}")),
        };
        perform(Effect::Pop, cx);
        perform(Effect::ShowToast(toast), cx);
    }

    fn value(&self, id: &str, saved: String) -> SharedString {
        match &self.draft {
            Some(draft) => text(draft, id).into(),
            None => saved.into(),
        }
    }

    fn with_error(&self, field: Field) -> Field {
        match self.errors.get(field.id().as_ref()) {
            Some(error) => field.with_error(*error),
            None => field,
        }
    }
}

impl Page for LayoutForm {
    fn title(&self) -> SharedString {
        match self.editing {
            Some(_) => "Edit Window Layout".into(),
            None => "Create Window Layout".into(),
        }
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let page = cx.entity().downgrade();
        let submit = FormHandler::new(move |values, _, cx| {
            page.update(cx, |page, cx| page.submit(values, cx)).ok();
        });
        let saved = self.editing.as_ref();
        let percent = |value: u16| {
            let value = f32::from(value) / 10.;
            match value.fract() == 0. {
                true => format!("{value:.0}"),
                false => format!("{value:.1}"),
            }
        };
        let saved_parts = saved.map(|layout| {
            [
                layout.frame.x,
                layout.frame.y,
                layout.frame.width,
                layout.frame.height,
            ]
        });
        let form = FormModel::new().with_field(self.with_error(Field::new(
            NAME,
            "Name",
            Control::Text {
                placeholder: Some("Left Two Fifths".into()),
                value: self.value(
                    NAME,
                    saved.map(|layout| layout.name.clone()).unwrap_or_default(),
                ),
            },
        )));
        FIELDS
            .iter()
            .enumerate()
            .fold(form, |form, (index, (id, title, placeholder))| {
                form.with_field(
                    self.with_error(Field::new(
                        *id,
                        *title,
                        Control::Text {
                            placeholder: Some((*placeholder).into()),
                            value: self.value(
                                id,
                                saved_parts
                                    .map(|parts| percent(parts[index]))
                                    .unwrap_or_else(|| (*placeholder).to_owned()),
                            ),
                        },
                    )),
                )
            })
            .with_actions(
                ActionPanel::new()
                    .with_action(Action::new("Save Layout", Effect::SubmitForm(submit))),
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
