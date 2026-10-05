//! A GNOME-style desktop shell prototype: a top bar on every display and a
//! dock, drawn by GPUI Kit on Wayland layer-shell surfaces.
//!
//! GPUI is a Wayland client, so the shell runs inside a compositor that
//! implements `wlr-layer-shell` (sway, Hyprland, niri, KDE, COSMIC) rather
//! than being one. See README.md for what it covers and what it does not.

#[cfg(target_os = "linux")]
mod apps;
#[cfg(target_os = "linux")]
mod assets;
#[cfg(target_os = "linux")]
mod calendar_popup;
#[cfg(target_os = "linux")]
mod compositor;
#[cfg(target_os = "linux")]
mod dock;
#[cfg(target_os = "linux")]
mod popup;
#[cfg(target_os = "linux")]
mod quick_settings;
#[cfg(target_os = "linux")]
mod system_status;
#[cfg(target_os = "linux")]
mod top_bar;

#[cfg(target_os = "linux")]
fn main() {
    shell::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The desktop example runs on Linux, inside a Wayland compositor with layer-shell.");
}

#[cfg(target_os = "linux")]
mod shell {
    use std::rc::Rc;
    use std::time::Duration;

    use gpui_kit::component::ActiveTheme as _;
    use gpui_kit::layer_shell::{
        Anchor, KeyboardInteractivity, Layer, LayerShellNotSupportedError, LayerShellOptions,
    };
    use gpui_kit::*;

    use crate::apps::AppCatalog;
    use crate::assets::DesktopAssets;
    use crate::compositor::Compositor;
    use crate::dock::{DOCK_HEIGHT, Dock};
    use crate::system_status::SystemStatus;
    use crate::top_bar::{TOP_BAR_HEIGHT, TopBar};

    const APP_ID: &str = "gpui-kit-desktop";

    pub fn run() {
        gpui_kit::application()
            .with_assets(DesktopAssets)
            .run(|cx| {
                gpui_kit::init(cx);

                // Wayland announces outputs as events, which the first
                // dispatches after start-up deliver; wait for them so each one
                // gets a bar.
                cx.spawn(async move |cx| {
                    for _ in 0..OUTPUT_WAIT_ATTEMPTS {
                        if !cx.update(|cx| cx.displays().is_empty()) {
                            break;
                        }
                        cx.background_executor().timer(OUTPUT_WAIT_STEP).await;
                    }
                    cx.update(open_shell);
                })
                .detach();
            });
    }

    const OUTPUT_WAIT_ATTEMPTS: usize = 40;
    const OUTPUT_WAIT_STEP: Duration = Duration::from_millis(25);

    fn open_shell(cx: &mut App) {
        let displays = cx.displays();
        if displays.is_empty() {
            eprintln!("desktop: the compositor reported no outputs.");
            cx.quit();
            return;
        }

        let compositor = cx.new(Compositor::new);
        let status = cx.new(SystemStatus::new);
        let catalog = Rc::new(AppCatalog::load());

        let result = displays
            .iter()
            .try_for_each(|display| {
                open_top_bar(
                    display.id(),
                    display.bounds().size.width,
                    compositor.clone(),
                    status.clone(),
                    cx,
                )
            })
            // Wayland has no primary output; the first one announced is the
            // closest stand-in.
            .and_then(|()| open_dock(displays[0].id(), catalog, compositor, cx));

        if let Err(err) = result {
            if err.downcast_ref::<LayerShellNotSupportedError>().is_some() {
                eprintln!(
                    "This compositor doesn't support wlr-layer-shell. Run the shell inside sway, \
                     Hyprland, niri, KDE Plasma or COSMIC."
                );
            } else {
                eprintln!("desktop: couldn't open the shell: {err:#}");
            }
            cx.quit();
        }
    }

    fn open_top_bar(
        display_id: DisplayId,
        width: Pixels,
        compositor: Entity<Compositor>,
        status: Entity<SystemStatus>,
        cx: &mut App,
    ) -> Result<()> {
        let height = cx.theme().font_size * TOP_BAR_HEIGHT;
        let options = WindowOptions {
            titlebar: None,
            focus: false,
            display_id: Some(display_id),
            app_id: Some(APP_ID.into()),
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(width, height),
            })),
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: format!("{APP_ID}-top-bar"),
                layer: Layer::Top,
                anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
                exclusive_zone: Some(height),
                keyboard_interactivity: KeyboardInteractivity::None,
                ..Default::default()
            }),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |_, cx| {
            cx.new(|cx| TopBar::new(compositor, status, cx))
        })?;
        Ok(())
    }

    fn open_dock(
        display_id: DisplayId,
        catalog: Rc<AppCatalog>,
        compositor: Entity<Compositor>,
        cx: &mut App,
    ) -> Result<()> {
        let rem = cx.theme().font_size;
        let surface = Dock::surface_size(catalog.favorites().len(), rem);
        let options = WindowOptions {
            titlebar: None,
            focus: false,
            display_id: Some(display_id),
            app_id: Some(APP_ID.into()),
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: surface,
            })),
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: format!("{APP_ID}-dock"),
                layer: Layer::Top,
                anchor: Anchor::BOTTOM,
                exclusive_zone: Some(rem * DOCK_HEIGHT),
                keyboard_interactivity: KeyboardInteractivity::None,
                ..Default::default()
            }),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Dock::new(catalog, compositor, window, cx))
        })?;
        Ok(())
    }
}
