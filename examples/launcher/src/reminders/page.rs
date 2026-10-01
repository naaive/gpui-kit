//! Search Reminders and the form that creates or edits one.

use std::collections::HashMap;

use anyhow::Result;
use chrono::{Local, TimeZone as _};
use gpui_kit::{App, AppContext as _, Context, Entity, SharedString, Subscription, Window};

use super::{Priority, Reminder, Reminders, due, format_due, priority_accessory, store, update};
use crate::{
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Choice,
        Confirmation, Control, DetailModel, Dropdown, Effect, Field, FormHandler, FormModel,
        FormValue, FormValues, Image, Item, ItemId, ListModel, PageModel, PushHandler, RunHandler,
        Section, Submenu, TextHandler, Toast, ToastStyle, Tone,
    },
    pages::{self, Page, PageHandle},
    shell::launcher::perform,
};

/// Which reminders the list shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Showing {
    #[default]
    Open,
    Completed,
    All,
}

impl Showing {
    const ALL: [Self; 3] = [Self::Open, Self::Completed, Self::All];

    fn value(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Completed => "completed",
            Self::All => "all",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Completed => "Completed",
            Self::All => "All Reminders",
        }
    }
}

pub fn search_reminders_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let store = store(cx);
    Ok(pages::handle(cx.new(|cx| RemindersPage {
        _subscription: cx.observe(&store, |page: &mut RemindersPage, _, cx| {
            page.list = None;
            cx.notify();
        }),
        store,
        showing: Showing::Open,
        list: None,
    })))
}

struct RemindersPage {
    store: Entity<Reminders>,
    showing: Showing,
    list: Option<ListModel>,
    _subscription: Subscription,
}

/// The snooze choices: a title and minutes from now, or `None` for
/// tomorrow morning.
const SNOOZES: [(&str, Option<i64>); 4] = [
    ("10 Minutes", Some(10)),
    ("1 Hour", Some(60)),
    ("3 Hours", Some(180)),
    ("Tomorrow Morning", None),
];

/// Minutes from now to 09:00 tomorrow.
fn minutes_to_tomorrow_morning() -> i64 {
    let now = Local::now();
    let morning = (now.date_naive() + chrono::Duration::days(1))
        .and_hms_opt(9, 0, 0)
        .and_then(|morning| Local.from_local_datetime(&morning).earliest())
        .unwrap_or(now + chrono::Duration::days(1));
    (morning - now).num_minutes().max(1)
}

fn item(reminder: &Reminder, now: chrono::DateTime<Local>) -> Item {
    let id = reminder.id.clone();
    let completed = reminder.is_completed();
    let toggle = id.clone();
    let delete = id.clone();
    let edit = reminder.clone();
    let snoozes = SNOOZES
        .iter()
        .fold(
            Submenu::new("Snooze").with_image(Image::Icon("alarm-clock".into())),
            |menu, (title, minutes)| {
                let (id, minutes) = (id.clone(), *minutes);
                menu.with_action(Action::new(
                    *title,
                    Effect::Run(RunHandler::new(move |(), _, cx| {
                        let minutes = minutes.unwrap_or_else(minutes_to_tomorrow_morning);
                        update(cx, |reminders, cx| reminders.snooze(&id, minutes, cx));
                    })),
                ))
            },
        )
        .with_shortcut("secondary-s");
    let mut actions = ActionPanel::new()
        .with_action(
            Action::new(
                match completed {
                    true => "Mark as Open",
                    false => "Complete Reminder",
                },
                Effect::Run(RunHandler::new(move |(), _, cx| {
                    update(cx, |reminders, cx| {
                        reminders.set_completed(&toggle, !completed, cx)
                    })
                })),
            )
            .with_image(Image::Icon(match completed {
                true => "circle".into(),
                false => "circle-check".into(),
            })),
        )
        .with_action(
            Action::new(
                "Edit Reminder",
                Effect::Push(PushHandler::new(move |window, cx| {
                    reminder_form(Some(edit.clone()), window, cx)
                })),
            )
            .with_image(Image::Icon("pencil".into()))
            .with_shortcut("secondary-e"),
        );
    if !completed {
        actions = actions.with_submenu(snoozes);
    }
    let actions = actions.with_action(
        Action::new("Copy Title", Effect::Copy(reminder.title.clone().into()))
            .with_image(Image::Icon("copy".into()))
            .with_shortcut("secondary-shift-c"),
    );
    let mut destructive = ActionSection::new();
    if completed {
        destructive = destructive.with_entry(ActionEntry::Action(
            Action::new(
                "Delete All Completed",
                Effect::Confirm(
                    Confirmation::new(
                        "Delete every completed reminder?",
                        Effect::Run(RunHandler::new(|(), _, cx| {
                            update(cx, |reminders, cx| reminders.clear_completed(cx))
                        })),
                    )
                    .with_confirm_title("Delete All")
                    .destructive(true),
                ),
            )
            .with_image(Image::Icon("trash-off".into()))
            .with_style(ActionStyle::Destructive),
        ));
    }
    let actions = actions
        .with_section(
            ActionSection::new()
                .with_entry(ActionEntry::Action(
                    Action::new(
                        "Create Reminder",
                        Effect::Push(PushHandler::new(create_reminder_page)),
                    )
                    .with_image(Image::Icon("plus".into()))
                    .with_shortcut("secondary-n"),
                ))
                .with_entry(ActionEntry::Action(
                    Action::new(
                        "Delete Reminder",
                        Effect::Confirm(
                            Confirmation::new(
                                "Delete this reminder?",
                                Effect::Run(RunHandler::new(move |(), _, cx| {
                                    update(cx, |reminders, cx| reminders.delete(&delete, cx))
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
        )
        .with_section(destructive);
    let mut item = Item::new(
        ItemId::new(format!("reminder/{}", reminder.id)),
        reminder.title.clone(),
    )
    .with_image(Image::Icon(match completed {
        true => "circle-check".into(),
        false => "circle".into(),
    }))
    .with_keyword(reminder.notes.clone())
    .with_actions(actions);
    if let Some(first) = reminder.notes.lines().find(|line| !line.trim().is_empty()) {
        item = item.with_subtitle(first.trim().to_owned());
    }
    if let Some(priority) = priority_accessory(reminder.priority) {
        item = item.with_accessory(priority);
    }
    if let Some(due) = reminder.due_time() {
        item = item.with_accessory(match !completed && due <= now {
            true => Accessory::tag(format_due(due, now), Tone::Danger),
            false => Accessory::text(format_due(due, now)),
        });
    }
    if !reminder.notes.trim().is_empty() {
        item = item.with_detail(DetailModel::new(reminder.notes.clone()));
    }
    item
}

impl RemindersPage {
    fn build_list(&self, cx: &mut Context<Self>) -> ListModel {
        let now = Local::now();
        let today = now.date_naive();
        let reminders = self.store.read(cx).reminders();
        let mut open: Vec<&Reminder> = reminders.iter().filter(|r| !r.is_completed()).collect();
        // Due ones by time, then undated ones by priority.
        open.sort_by_key(|reminder| {
            (
                reminder.due.is_none(),
                reminder.due,
                std::cmp::Reverse(reminder.priority as u8),
            )
        });
        let mut completed: Vec<&Reminder> = reminders.iter().filter(|r| r.is_completed()).collect();
        completed.sort_by_key(|reminder| std::cmp::Reverse(reminder.completed_at));

        let mut groups: Vec<(&str, Vec<Item>)> = vec![
            ("Overdue", Vec::new()),
            ("Today", Vec::new()),
            ("Upcoming", Vec::new()),
            ("No Due Date", Vec::new()),
        ];
        if self.showing != Showing::Completed {
            for reminder in open {
                let group = match reminder.due_time() {
                    Some(due) if due <= now => 0,
                    Some(due) if due.date_naive() == today => 1,
                    Some(_) => 2,
                    None => 3,
                };
                groups[group].1.push(item(reminder, now));
            }
        }
        if self.showing != Showing::Open {
            groups.push((
                "Completed",
                completed
                    .into_iter()
                    .map(|reminder| item(reminder, now))
                    .collect(),
            ));
        }
        let page = cx.entity().downgrade();
        let dropdown = Showing::ALL.into_iter().fold(
            Dropdown::new("Show")
                .with_value(self.showing.value())
                .with_on_change(TextHandler::new(move |value, _, cx| {
                    page.update(cx, |page, cx| {
                        page.showing = Showing::ALL
                            .into_iter()
                            .find(|showing| showing.value() == value.as_ref())
                            .unwrap_or_default();
                        page.list = None;
                        cx.notify();
                    })
                    .ok();
                })),
            |dropdown, showing| dropdown.with_choice(Choice::new(showing.value(), showing.title())),
        );
        let create = (self.showing != Showing::Completed).then(|| {
            Item::new(ItemId::new("reminder/create"), "Create Reminder")
                .with_image(Image::Icon("plus".into()))
                .with_keyword("new")
                .with_action(Action::new(
                    "Create Reminder",
                    Effect::Push(PushHandler::new(create_reminder_page)),
                ))
        });
        groups.into_iter().fold(
            ListModel::new()
                .with_placeholder("Search reminders…")
                .with_dropdown(dropdown)
                .with_section(Section::new().with_items(create))
                .with_empty_title("No completed reminders"),
            |list, (title, items)| {
                list.with_section(Section::new().with_title(title).with_items(items))
            },
        )
    }
}

impl Page for RemindersPage {
    fn title(&self) -> SharedString {
        "Reminders".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let list = match self.list.take() {
            Some(list) => list,
            None => self.build_list(cx),
        };
        self.list = Some(list.clone());
        PageModel::List(list)
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

const TITLE: &str = "title";
const NOTES: &str = "notes";
const DUE: &str = "due";
const PRIORITY: &str = "priority";

pub fn create_reminder_page(window: &mut Window, cx: &mut App) -> Result<PageHandle> {
    reminder_form(None, window, cx)
}

/// Creates a reminder, or edits `editing`.
pub fn reminder_form(
    editing: Option<Reminder>,
    _: &mut Window,
    cx: &mut App,
) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|_| ReminderForm {
        editing,
        draft: None,
        errors: HashMap::new(),
        due_preview: None,
    })))
}

struct ReminderForm {
    editing: Option<Reminder>,
    draft: Option<FormValues>,
    errors: HashMap<&'static str, &'static str>,
    /// When the typed due time reads as, shown under the field.
    due_preview: Option<String>,
}

fn text(values: &FormValues, id: &str) -> String {
    match values.get(id) {
        Some(FormValue::Text(text)) => text.trim().to_owned(),
        _ => String::new(),
    }
}

/// The due time `text` reads as, in Unix seconds.
fn parse_due(text: &str) -> Option<i64> {
    let due = due::parse(text, Local::now().naive_local())?;
    Local
        .from_local_datetime(&due)
        .earliest()
        .map(|due| due.timestamp())
}

impl ReminderForm {
    fn submit(&mut self, values: FormValues, cx: &mut Context<Self>) {
        self.errors.clear();
        let title = text(&values, TITLE);
        if title.is_empty() {
            self.errors.insert(TITLE, "Give the reminder a title");
        }
        let due_text = text(&values, DUE);
        let due = match due_text.is_empty() {
            true => None,
            false => match parse_due(&due_text) {
                Some(due) => Some(due),
                None => {
                    self.errors.insert(
                        DUE,
                        "Try “tomorrow 9am”, “in 2 hours” or “2026-10-03 14:30”",
                    );
                    None
                }
            },
        };
        if !self.errors.is_empty() {
            self.draft = Some(values);
            cx.notify();
            return;
        }
        let reminder = Reminder {
            id: self
                .editing
                .as_ref()
                .map(|editing| editing.id.clone())
                .unwrap_or_else(|| Local::now().format("%Y%m%d%H%M%S%3f").to_string()),
            title: title.clone(),
            notes: text(&values, NOTES),
            due,
            priority: Priority::parse(&text(&values, PRIORITY)),
            completed_at: self
                .editing
                .as_ref()
                .and_then(|editing| editing.completed_at),
            alerted: self.editing.as_ref().is_some_and(|editing| editing.alerted),
        };
        let message = match &reminder.due_time() {
            Some(due) => format!("Due {}", format_due(*due, Local::now())),
            None => "No due date".to_owned(),
        };
        update(cx, |reminders, cx| reminders.save_reminder(reminder, cx));
        perform(Effect::Pop, cx);
        perform(
            Effect::ShowToast(
                Toast::new(ToastStyle::Success, format!("Saved “{title}”")).with_message(message),
            ),
            cx,
        );
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

impl Page for ReminderForm {
    fn title(&self) -> SharedString {
        match self.editing {
            Some(_) => "Edit Reminder".into(),
            None => "Create Reminder".into(),
        }
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let page = cx.entity().downgrade();
        let submit = FormHandler::new(move |values, _, cx| {
            page.update(cx, |page, cx| page.submit(values, cx)).ok();
        });
        let preview_page = cx.entity().downgrade();
        let saved = self.editing.as_ref();
        let saved_due = saved
            .and_then(Reminder::due_time)
            .map(|due| due.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let due_info = self
            .due_preview
            .clone()
            .unwrap_or_else(|| "“tomorrow 9am”, “in 2 hours”, “fri 17:00”, “明天下午3点”".into());
        FormModel::new()
            .with_field(
                self.with_error(Field::new(
                    TITLE,
                    "Title",
                    Control::Text {
                        placeholder: Some("Call the dentist".into()),
                        value: self.value(
                            TITLE,
                            saved
                                .map(|reminder| reminder.title.clone())
                                .unwrap_or_default(),
                        ),
                    },
                )),
            )
            .with_field(
                self.with_error(
                    Field::new(
                        DUE,
                        "Due",
                        Control::Text {
                            placeholder: Some("tomorrow 9am".into()),
                            value: self.value(DUE, saved_due),
                        },
                    )
                    .with_info(due_info)
                    .with_on_change(crate::model::Callback::new(
                        move |value: FormValue, _, cx| {
                            let FormValue::Text(text) = value else {
                                return;
                            };
                            let preview = match text.trim().is_empty() {
                                true => None,
                                false => Some(match parse_due(&text) {
                                    Some(due) => match Local.timestamp_opt(due, 0).single() {
                                        Some(due) => due.format("%A, %B %-d at %H:%M").to_string(),
                                        None => String::new(),
                                    },
                                    None => "Not a time this understands".into(),
                                }),
                            };
                            preview_page
                                .update(cx, |page, cx| {
                                    if page.due_preview != preview {
                                        page.due_preview = preview;
                                        cx.notify();
                                    }
                                })
                                .ok();
                        },
                    )),
                ),
            )
            .with_field(Field::new(
                PRIORITY,
                "Priority",
                Control::Dropdown {
                    choices: Priority::ALL
                        .into_iter()
                        .map(|priority| Choice::new(priority.value(), priority.title()))
                        .collect(),
                    value: Some(
                        match &self.draft {
                            Some(draft) => text(draft, PRIORITY),
                            None => saved
                                .map(|reminder| reminder.priority)
                                .unwrap_or_default()
                                .value()
                                .to_owned(),
                        }
                        .into(),
                    ),
                },
            ))
            .with_field(Field::new(
                NOTES,
                "Notes",
                Control::TextArea {
                    placeholder: Some("Optional".into()),
                    value: self.value(
                        NOTES,
                        saved
                            .map(|reminder| reminder.notes.clone())
                            .unwrap_or_default(),
                    ),
                },
            ))
            .with_actions(
                ActionPanel::new()
                    .with_action(Action::new("Save Reminder", Effect::SubmitForm(submit))),
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
