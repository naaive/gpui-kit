//! Asks for a date, or a date and time, and hands it to whoever asked: an
//! extension's `Action.pick_date`.

use gpui_kit::{App, AppContext as _, Context, SharedString, Window};

use super::{Page, PageHandle};
use crate::model::{
    Action, ActionPanel, Control, Effect, Field, FormHandler, FormModel, FormValue, PageModel,
    TextHandler,
};

const DATE: &str = "date";

pub struct PickDatePage {
    title: SharedString,
    include_time: bool,
    on_pick: TextHandler,
}

/// A page with one date field; choosing returns to the page that asked and
/// calls `on_pick` with the value, `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM`.
pub fn pick_date_page(
    title: impl Into<SharedString>,
    include_time: bool,
    on_pick: TextHandler,
    cx: &mut App,
) -> PageHandle {
    super::handle(cx.new(|_| PickDatePage {
        title: title.into(),
        include_time,
        on_pick,
    }))
}

impl Page for PickDatePage {
    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
        let control = match self.include_time {
            true => Control::DateTime { value: None },
            false => Control::Date { value: None },
        };
        let on_pick = self.on_pick.clone();
        let choose = FormHandler::new(move |values, window, cx| {
            let Some(FormValue::Text(value)) = values.get(DATE).cloned() else {
                return;
            };
            crate::shell::launcher::perform(Effect::Pop, cx);
            on_pick.call(value, window, cx);
        });
        FormModel::new()
            .with_field(Field::new(DATE, "Date", control))
            .with_actions(
                ActionPanel::new().with_action(Action::new("Choose", Effect::SubmitForm(choose))),
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
