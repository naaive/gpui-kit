use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon,
    button::Button,
    dock::{BasePanel, Panel, PanelControl, PanelEvent},
    h_flex,
    kbd::Kbd,
    v_flex,
};
use gpui_kit::{
    App, Context, EventEmitter, FocusHandle, Focusable, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, prelude::FluentBuilder as _,
};
use rust_i18n::t;

use super::{NewConsole, NewDataSource};
use crate::{console::ExecuteStatement, datasource::DataSources};

/// What the center shows while no console is open: the next step.
pub struct StartPanel {
    focus_handle: FocusHandle,
}

impl EventEmitter<PanelEvent> for StartPanel {}

impl StartPanel {
    pub const NAME: &str = "Start";

    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Focusable for StartPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for StartPanel {
    fn panel_name(&self) -> &'static str {
        Self::NAME
    }

    fn closable(&self, _: &App) -> bool {
        false
    }

    fn zoomable(&self, _: &App) -> bool {
        false
    }
}

impl Panel for StartPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        t!("start.title").to_string()
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        None
    }

    fn title_bar(&self, _: &App) -> bool {
        false
    }
}

impl Render for StartPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_data_sources = !DataSources::global(cx).read(cx).items().is_empty();
        let shortcut = |label: String, kbd: Option<Kbd>| {
            h_flex()
                .w_full()
                .justify_between()
                .gap_4()
                .child(label)
                .children(kbd)
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_6()
            .p_8()
            .child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::DatabaseZap)
                            .size_10()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(div().text_lg().child(t!("start.heading").to_string()))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(if has_data_sources {
                                t!("start.open_console_hint").to_string()
                            } else {
                                t!("start.add_data_source_hint").to_string()
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("start-new-data-source")
                            .icon(IconName::Plus)
                            .label(t!("start.new_data_source").to_string())
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(NewDataSource), cx)
                            }),
                    )
                    .when(has_data_sources, |row| {
                        row.child(
                            Button::new("start-new-console")
                                .icon(IconName::SquareTerminal)
                                .label(t!("start.new_console").to_string())
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(NewConsole), cx)
                                }),
                        )
                    }),
            )
            .child(
                v_flex()
                    .w_64()
                    .gap_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(shortcut(
                        t!("start.shortcut_new_console").to_string(),
                        Kbd::binding_for_action(&NewConsole, None, window),
                    ))
                    .child(shortcut(
                        t!("start.shortcut_execute").to_string(),
                        Kbd::binding_for_action(
                            &ExecuteStatement,
                            Some(crate::console::CONTEXT),
                            window,
                        ),
                    )),
            )
    }
}
