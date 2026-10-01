//! What happens to a capture once the session ends: copy, save, save as,
//! or pin. The image is composed on a background thread first.

pub mod clipboard;

use std::sync::Arc;

use anyhow::{Context as _, Result};
use gpui_kit::{App, AppContext as _, AsyncApp};
use image::RgbaImage;

use crate::{
    app,
    capture::Frame,
    geometry::PhysRect,
    pin,
    raster::{self, ImageFormat, TileCache},
    scene::Scene,
    session::machine::Outcome,
    shell::hud,
};

/// Everything needed to produce the image of a finished session.
pub struct Delivery {
    frame: Frame,
    selection: PhysRect,
    scene: Scene,
    tiles: TileCache,
}

impl Delivery {
    pub fn new(frame: Frame, selection: PhysRect, scene: Scene, tiles: TileCache) -> Self {
        Self {
            frame,
            selection,
            scene,
            tiles,
        }
    }
}

/// Composes the capture and does what `outcome` asks with it.
pub fn deliver(outcome: Outcome, delivery: Delivery, cx: &mut App) {
    let Delivery {
        frame,
        selection,
        scene,
        mut tiles,
    } = delivery;
    if outcome == Outcome::ScrollCapture {
        scroll_capture(selection, frame.area(), frame.native_id(), cx);
        return;
    }
    let area = frame.area();
    let native_id = frame.native_id();
    let composed = cx.background_spawn(async move {
        raster::compose(&frame, selection, &scene, &mut tiles).map(Arc::new)
    });
    cx.spawn(async move |cx| {
        let image = match composed.await {
            Ok(image) => image,
            Err(error) => {
                report(cx, format!("Couldn’t create the image: {error:#}"));
                return;
            }
        };
        let result = match outcome {
            Outcome::Copy => copy(&image, cx).await,
            Outcome::CopyText => copy_text(image, cx).await,
            // Handled before composing; see `deliver`.
            Outcome::ScrollCapture => Ok(()),
            Outcome::Save => save(image, cx).await,
            Outcome::SaveAs => save_as(image, cx).await,
            Outcome::Pin => cx.update(|cx| {
                pin::open(
                    image,
                    pin::Placement::new(selection.origin(), area, native_id),
                    cx,
                )
            }),
            Outcome::Cancel => Ok(()),
        };
        if let Err(error) = result {
            report(cx, format!("{error:#}"));
        }
    })
    .detach();
}

fn report(cx: &mut AsyncApp, message: String) {
    tracing::error!("{message}");
    cx.update(|cx| hud::show(message, cx));
}

async fn copy(image: &Arc<RgbaImage>, cx: &mut AsyncApp) -> Result<()> {
    let clipboard = cx.update(|cx| clipboard::clipboard(cx));
    let image = image.clone();
    cx.background_spawn(async move { clipboard.write_image(&image) })
        .await
}

/// Scrolls the content under `selection` and joins it, then copies the
/// result and pins it, where it can be looked over and saved.
fn scroll_capture(
    selection: PhysRect,
    area: crate::geometry::DisplayArea,
    native_id: u64,
    cx: &mut App,
) {
    let capturer = app::capturer(cx);
    cx.spawn(async move |cx| {
        let captured = cx
            .background_spawn(async move { crate::scroll::capture(&*capturer, selection) })
            .await;
        let image = match captured {
            Ok(image) => Arc::new(image),
            Err(error) => {
                report(cx, format!("{error:#}"));
                return;
            }
        };
        if let Err(error) = copy(&image, cx).await {
            report(cx, format!("{error:#}"));
        }
        let (width, height) = image.dimensions();
        let pinned = cx.update(|cx| {
            pin::open(
                image,
                pin::Placement::new(selection.origin(), area, native_id),
                cx,
            )
        });
        match pinned {
            Ok(()) => cx.update(|cx| hud::show(format!("Copied {width} × {height}"), cx)),
            Err(error) => report(cx, format!("{error:#}")),
        }
    })
    .detach();
}

/// Reads the text in the capture and copies it, saying what was found.
async fn copy_text(image: Arc<RgbaImage>, cx: &mut AsyncApp) -> Result<()> {
    let reading = cx
        .background_spawn(async move { crate::recognize::read(&image) })
        .await?;
    let Some(text) = reading.text() else {
        cx.update(|cx| hud::show("No text found", cx));
        return Ok(());
    };
    let clipboard = cx.update(|cx| clipboard::clipboard(cx));
    let copied = text.clone();
    cx.background_spawn(async move { clipboard.write_text(&copied) })
        .await?;
    let message = match &reading {
        crate::recognize::Reading::QrCodes(codes) if codes.len() > 1 => {
            format!("Copied {} QR codes", codes.len())
        }
        crate::recognize::Reading::QrCodes(_) => format!("Copied QR code: {}", first_line(&text)),
        _ => format!("Copied: {}", first_line(&text)),
    };
    cx.update(|cx| hud::show(message, cx));
    Ok(())
}

/// The first line of `text`, marked when more follows.
fn first_line(text: &str) -> String {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let first = lines.next().unwrap_or_default().trim().to_owned();
    if lines.next().is_some() {
        format!("{first} …")
    } else {
        first
    }
}

/// Saves to the save folder under a name from the template.
async fn save(image: Arc<RgbaImage>, cx: &mut AsyncApp) -> Result<()> {
    let settings = cx.update(|cx| app::settings(cx).clone());
    let path = raster::unused_path(
        &settings.save_directory(),
        settings.name_template(),
        settings.image_format(),
        chrono::Local::now(),
    )?;
    let written = path.clone();
    let format = settings.image_format();
    let saved = image.clone();
    cx.background_spawn(async move { raster::write(&saved, format, &written) })
        .await?;
    if settings.is_copying_after_saving() {
        copy(&image, cx).await?;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    cx.update(|cx| hud::show(format!("Saved {name}"), cx));
    Ok(())
}

/// Asks where to save, starting in the save folder.
async fn save_as(image: Arc<RgbaImage>, cx: &mut AsyncApp) -> Result<()> {
    let settings = cx.update(|cx| app::settings(cx).clone());
    let directory = settings.save_directory();
    std::fs::create_dir_all(&directory).ok();
    let suggested = format!(
        "{}.{}",
        raster::file_name(settings.name_template(), chrono::Local::now())?,
        settings.image_format().extension()
    );
    let chosen = cx
        .update(|cx| cx.prompt_for_new_path(&directory, Some(&suggested)))
        .await
        .context("the save dialog closed unexpectedly")??;
    let Some(path) = chosen else {
        return Ok(());
    };
    let format = ImageFormat::from_path(&path).unwrap_or(settings.image_format());
    let path = if ImageFormat::from_path(&path).is_some() {
        path.to_path_buf()
    } else {
        path.with_extension(format.extension())
    };
    cx.background_spawn(async move { raster::write(&image, format, &path) })
        .await
}

/// Copies an image, reporting a failure.
pub fn copy_image(image: Arc<RgbaImage>, cx: &mut App) {
    cx.spawn(async move |cx| {
        if let Err(error) = copy(&image, cx).await {
            report(cx, format!("{error:#}"));
        }
    })
    .detach();
}

/// Asks where to save an image and saves it there, reporting a failure.
pub fn save_image_as(image: Arc<RgbaImage>, cx: &mut App) {
    cx.spawn(async move |cx| {
        if let Err(error) = save_as(image, cx).await {
            report(cx, format!("{error:#}"));
        }
    })
    .detach();
}
