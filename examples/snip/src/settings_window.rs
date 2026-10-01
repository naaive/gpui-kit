//! The settings window, opened from the tray or `snip settings`.
//!
//! Every field applies as soon as it changes; there is no Save button.

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::Button,
    h_flex,
    kbd::Kbd,
    setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings as SettingsView},
    v_flex,
};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Axis, Bounds, Context, Global, IntoElement, Keystroke,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, TitlebarOptions,
    Window, WindowBounds, WindowOptions, div, px, size,
};

use crate::{
    app,
    raster::{self, ImageFormat},
    shell::{
        hotkey::{self, HotkeyCommand, HotkeyStatus},
        settings::{Appearance, Settings},
    },
};

#[derive(Default)]
struct OpenSettings(Option<AnyWindowHandle>);

impl Global for OpenSettings {}

pub fn init(cx: &mut App) {
    cx.set_global(OpenSettings::default());
}

/// Shows the settings window, opening it if needed.
pub fn open(cx: &mut App) {
    if let Some(handle) = cx.global::<OpenSettings>().0
        && cx.windows().contains(&handle)
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        cx.activate(true);
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(760.), px(560.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Snip settings".into()),
            ..Default::default()
        }),
        window_min_size: Some(size(px(560.), px(400.))),
        app_id: Some("snip".into()),
        show: !app::is_offscreen(cx),
        focus: !app::is_offscreen(cx),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| SettingsWindow)) {
        Ok((handle, _)) => {
            cx.activate(true);
            cx.global_mut::<OpenSettings>().0 = Some(handle);
        }
        Err(error) => tracing::error!("cannot open the settings window: {error:#}"),
    }
}

/// The settings window, if open.
#[cfg_attr(not(feature = "preview"), allow(dead_code))]
pub fn handle(cx: &App) -> Option<AnyWindowHandle> {
    cx.global::<OpenSettings>().0
}

struct SettingsWindow;

/// Applies `change` to the settings.
fn change(cx: &mut App, change: impl FnOnce(Settings) -> Settings) {
    let settings = change(app::settings(cx).clone());
    app::update_settings(settings, None, cx);
}

fn shortcut_description(command: HotkeyCommand, shortcut: &str, cx: &App) -> SharedString {
    if let Err(error) = hotkey::parse_shortcut(shortcut) {
        return error.to_string().into();
    }
    match app::hotkey_status(command, cx) {
        HotkeyStatus::Registered => "Works in every application.".into(),
        status => status.to_string().into(),
    }
}

/// The keys that work while selecting and annotating, for reference.
const CAPTURE_KEYS: [(&str, &str); 10] = [
    ("enter", "Copy and close"),
    ("secondary-s", "Save to the save folder"),
    ("secondary-shift-s", "Save as…"),
    ("f3", "Pin to the screen"),
    ("escape", "Step back, or cancel"),
    ("secondary-z", "Undo"),
    ("left", "Move the selection by 1 pixel; hold Shift for 10"),
    ("secondary-right", "Grow or shrink the selection"),
    ("c", "Copy the color under the magnifier"),
    (
        "r",
        "Rectangle; also E, A, L, P, M, X, T and N for the other tools",
    ),
];

impl SettingsWindow {
    fn pages(&self, cx: &App) -> Vec<SettingPage> {
        let settings = app::settings(cx);
        let capture_shortcut = settings.capture_shortcut().to_owned();
        let pin_shortcut = settings.pin_shortcut().to_owned();
        let template = settings.name_template().to_owned();
        let example = raster::file_name(&template, chrono::Local::now())
            .map(|name| {
                format!(
                    "Images are named like “{name}.{}”.",
                    settings.image_format().extension()
                )
            })
            .unwrap_or_else(|error| error.to_string());
        let save_directory = settings.save_directory();
        let muted = cx.theme().muted_foreground;

        vec![
            SettingPage::new("General")
                .icon(IconName::Settings2)
                .groups(vec![
                    SettingGroup::new()
                        .title("Appearance")
                        .item(SettingItem::new(
                            "Theme",
                            SettingField::dropdown(
                                Appearance::ALL
                                    .iter()
                                    .map(|appearance| {
                                        (appearance.title().into(), appearance.title().into())
                                    })
                                    .collect(),
                                |cx: &App| app::settings(cx).appearance().title().into(),
                                |value: SharedString, cx: &mut App| {
                                    if let Some(appearance) = Appearance::ALL
                                        .into_iter()
                                        .find(|appearance| appearance.title() == value.as_ref())
                                    {
                                        change(cx, |settings| settings.with_appearance(appearance));
                                        app::apply_appearance(None, cx);
                                    }
                                },
                            ),
                        )),
                    SettingGroup::new().title("Capture").items(vec![
                        SettingItem::new(
                            "Detect windows",
                            SettingField::switch(
                                |cx: &App| app::settings(cx).is_detecting_windows(),
                                |value: bool, cx: &mut App| {
                                    change(cx, |settings| settings.with_detect_windows(value));
                                },
                            )
                            .default_value(true),
                        )
                        .description("Highlight the window under the pointer; click to select it."),
                        SettingItem::new(
                            "Show magnifier",
                            SettingField::switch(
                                |cx: &App| app::settings(cx).is_showing_magnifier(),
                                |value: bool, cx: &mut App| {
                                    change(cx, |settings| settings.with_show_magnifier(value));
                                },
                            )
                            .default_value(true),
                        )
                        .description("Enlarge the pixels around the pointer while selecting."),
                    ]),
                ]),
            SettingPage::new("Output")
                .icon(IconName::ImageDown)
                .groups(vec![SettingGroup::new().title("Saving").items(vec![
                        SettingItem::new(
                            "Save folder",
                            SettingField::render(move |options, _, _| {
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(muted)
                                            .max_w(px(260.))
                                            .truncate()
                                            .child(save_directory.display().to_string()),
                                    )
                                    .child(
                                        Button::new("choose-folder")
                                            .outline()
                                            .label("Choose…")
                                            .with_size(options.size())
                                            .on_click(|_, _, cx| choose_save_directory(cx)),
                                    )
                            }),
                        )
                        .description("Where Save puts images."),
                        SettingItem::new(
                            "File name",
                            SettingField::input(
                                |cx: &App| app::settings(cx).name_template().to_owned().into(),
                                |value: SharedString, cx: &mut App| {
                                    if raster::file_name(&value, chrono::Local::now()).is_ok() {
                                        change(cx, |settings| {
                                            settings.with_name_template(value.to_string())
                                        });
                                    }
                                },
                            )
                            .default_value(raster::DEFAULT_NAME_TEMPLATE),
                        )
                        .layout(Axis::Vertical)
                        .description(format!(
                            "{example} Use strftime codes such as %Y-%m-%d for the date."
                        )),
                        SettingItem::new(
                            "Format",
                            SettingField::dropdown(
                                ImageFormat::ALL
                                    .iter()
                                    .map(|format| (format.title().into(), format.title().into()))
                                    .collect(),
                                |cx: &App| app::settings(cx).image_format().title().into(),
                                |value: SharedString, cx: &mut App| {
                                    if let Some(format) = ImageFormat::ALL
                                        .into_iter()
                                        .find(|format| format.title() == value.as_ref())
                                    {
                                        change(cx, |settings| settings.with_image_format(format));
                                    }
                                },
                            ),
                        ),
                        SettingItem::new(
                            "Copy after saving",
                            SettingField::switch(
                                |cx: &App| app::settings(cx).is_copying_after_saving(),
                                |value: bool, cx: &mut App| {
                                    change(cx, |settings| settings.with_copy_after_saving(value));
                                },
                            )
                            .default_value(false),
                        )
                        .description("Also put saved images on the clipboard."),
                    ])]),
            SettingPage::new("Shortcuts")
                .icon(IconName::Keyboard)
                .groups(vec![
                    SettingGroup::new().title("System-wide").items(vec![
                        SettingItem::new(
                            "Capture",
                            SettingField::input(
                                |cx: &App| app::settings(cx).capture_shortcut().to_owned().into(),
                                |value: SharedString, cx: &mut App| {
                                    change(cx, |settings| {
                                        settings.with_capture_shortcut(value.trim())
                                    });
                                },
                            )
                            .default_value(crate::shell::settings::DEFAULT_CAPTURE_SHORTCUT),
                        )
                        .description(shortcut_description(
                            HotkeyCommand::Capture,
                            &capture_shortcut,
                            cx,
                        )),
                        SettingItem::new(
                            "Pin clipboard",
                            SettingField::input(
                                |cx: &App| app::settings(cx).pin_shortcut().to_owned().into(),
                                |value: SharedString, cx: &mut App| {
                                    change(cx, |settings| settings.with_pin_shortcut(value.trim()));
                                },
                            )
                            .default_value(crate::shell::settings::DEFAULT_PIN_SHORTCUT),
                        )
                        .description(shortcut_description(
                            HotkeyCommand::PinClipboard,
                            &pin_shortcut,
                            cx,
                        )),
                    ]),
                    SettingGroup::new()
                        .title("While capturing")
                        .item(SettingItem::render(move |_, _, cx| {
                            v_flex()
                                .w_full()
                                .gap_2()
                                .children(CAPTURE_KEYS.iter().map(|(key, meaning)| {
                                    h_flex()
                                        .gap_3()
                                        .child(
                                            div().w(px(140.)).child(
                                                Keystroke::parse(key)
                                                    .map(|keystroke| {
                                                        Kbd::new(keystroke).into_any_element()
                                                    })
                                                    .unwrap_or_else(|_| {
                                                        div().child(*key).into_any_element()
                                                    }),
                                            ),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(*meaning),
                                        )
                                }))
                                .into_any_element()
                        })),
                ]),
        ]
    }
}

/// Asks for a new save folder.
fn choose_save_directory(cx: &mut App) {
    let chosen = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Choose".into()),
    });
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(paths))) = chosen.await
            && let Some(directory) = paths.into_iter().next()
        {
            cx.update(|cx| {
                change(cx, |settings| settings.with_save_directory(Some(directory)));
            });
        }
    })
    .detach();
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        SettingsView::new("snip-settings")
            .sidebar_width(px(180.))
            .pages(self.pages(cx))
    }
}
