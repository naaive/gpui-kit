//! `snip --render-preview <directory>`: the interface as images.
//!
//! Captures the real screen, then plays a session on hidden windows with
//! synthesized input — select an area, draw with several tools, pin the
//! result — and writes what each window shows at each step as PNG files.
//! Nothing appears on screen and the clipboard is left alone, so the
//! interface can be reviewed, in both themes, on a machine in use.
//!
//! Built only with the `preview` feature, which needs GPUI's test support
//! for offscreen rendering and input synthesis.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, Result, bail};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, App, AppContext as _, AsyncApp, Window, point, px};

use crate::{app, capture, geometry::PhysPoint, pin, session, settings_window};

pub fn run(directory: PathBuf) {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        // Windows open and close throughout; the preview quits when done.
        .with_quit_mode(gpui_kit::QuitMode::Explicit)
        .run(move |cx| {
            gpui_kit::init(cx);
            app::init(cx);
            app::start(app::Startup::offscreen(), cx);
            cx.spawn(async move |cx| {
                for mode in [ThemeMode::Light, ThemeMode::Dark] {
                    if let Err(error) = render(&directory, mode, cx).await {
                        eprintln!("snip: preview failed: {error:#}");
                        break;
                    }
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}

async fn render(directory: &std::path::Path, mode: ThemeMode, cx: &mut AsyncApp) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let suffix = match mode {
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    };
    cx.update(|cx| Theme::change(mode, None, cx));
    let save = |window: &mut Window, name: &str| -> Result<()> {
        let image = window.render_to_image()?;
        let path = directory.join(format!("{name}-{suffix}.png"));
        image.save(&path)?;
        println!("{}", path.display());
        Ok(())
    };

    // A capture with the pointer a third of the way into the first display,
    // so the magnifier and hover show without depending on the real pointer.
    let capturer = cx.update(|cx| app::capturer(cx));
    let capture = cx
        .background_spawn(async move { capture::capture_all(&*capturer, true) })
        .await?;
    let first_area = capture.frames().first().context("no display")?.area();
    let first = first_area.bounds();
    let pointer = PhysPoint::new(first.x + first.width / 3, first.y + first.height / 3);
    let capture = capture.with_pointer(Some(pointer));
    let images = session::render_images(&capture);
    cx.update(|cx| session::open_with(capture, images, cx))?;
    let overlay = cx
        .update(|cx| session::overlay_windows(cx))
        .last()
        .copied()
        .context("no overlay opened")?;
    settle(cx).await;

    // Positions in thousandths of the display's logical width and height.
    let logical_width = first.width as f32 / first_area.scale();
    let logical_height = first.height as f32 / first_area.scale();
    let at = |x: f32, y: f32| {
        point(
            px(x / 1000. * logical_width),
            px(y / 1000. * logical_height),
        )
    };
    update(overlay, cx, |window, cx| {
        window.render_frame(cx);
        save(window, "1-selecting")
    })?;
    update(overlay, cx, |window, cx| {
        window.drag(at(150., 120.), at(700., 520.), cx);
        window.render_frame(cx);
        save(window, "2-selected")
    })?;
    update(overlay, cx, |window, cx| {
        window.press("r", cx);
        window.drag(at(190., 160.), at(380., 300.), cx);
        window.press("a", cx);
        window.drag(at(600., 470.), at(420., 320.), cx);
        window.press("m", cx);
        window.drag(at(200., 360.), at(560., 360.), cx);
        window.press("x", cx);
        window.drag(at(450., 160.), at(650., 200.), cx);
        window.press("n", cx);
        window.drag(at(200., 450.), at(200., 450.), cx);
        window.drag(at(260., 450.), at(260., 450.), cx);
        window.press("t", cx);
        window.drag(at(300., 440.), at(300., 440.), cx);
        window.input("Ship it", cx);
        window.render_frame(cx);
        save(window, "3a-typing")?;
        window.press("enter", cx);
        window.press("e", cx);
        // Pick the rectangle by its edge, to show the selected mark.
        window.drag(at(190., 230.), at(190., 230.), cx);
        window.render_frame(cx);
        save(window, "3-annotated")
    })?;
    // Pinning ends the session, closing the overlay mid-update.
    update(overlay, cx, |window, cx| {
        window.press("f3", cx);
        Ok(())
    })
    .ok();

    // Composing runs in the background; wait for the pin to open.
    let mut pin_window = None;
    for _ in 0..100 {
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
        pin_window = cx.update(|cx| pin::handles(cx)).last().copied();
        if pin_window.is_some() {
            break;
        }
    }
    let pin_window = pin_window.context("the pin never opened")?;
    settle(cx).await;
    update(pin_window, cx, |window, cx| {
        window.render_frame(cx);
        save(window, "4-pin")
    })?;
    cx.update(pin::close_all);

    cx.update(settings_window::open);
    let settings = cx
        .update(|cx| settings_window::handle(cx))
        .context("no settings window")?;
    update(settings, cx, |window, _| {
        window.resize(gpui_kit::size(px(760.), px(560.)));
        Ok(())
    })?;
    settle(cx).await;
    // The page list lays out over a few frames.
    update(settings, cx, |window, cx| {
        window.render_frame(cx);
        Ok(())
    })?;
    settle(cx).await;
    update(settings, cx, |window, cx| {
        window.render_frame(cx);
        save(window, "5-settings")
    })?;
    update(settings, cx, |window, _| {
        window.remove_window();
        Ok(())
    })?;
    Ok(())
}

/// Windows sizes a hidden window asynchronously; this lets it settle.
async fn settle(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(300))
        .await;
}

fn update(
    handle: AnyWindowHandle,
    cx: &mut AsyncApp,
    f: impl FnOnce(&mut Window, &mut App) -> Result<()>,
) -> Result<()> {
    match handle.update(cx, |_, window, cx| f(window, cx)) {
        Ok(result) => result,
        Err(error) => bail!("the window closed: {error}"),
    }
}
