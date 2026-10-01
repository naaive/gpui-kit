//! The small visual vocabulary every region shares: pictures and tags.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
};

use gpui_kit::{
    AnyElement, Hsla, IntoElement, ObjectFit, ParentElement as _, SharedString, SharedUri,
    Styled as _, StyledImage as _,
    component::{Icon, Sizable as _, Theme, tag::Tag},
    div, img,
    prelude::FluentBuilder as _,
    px,
};

use crate::model::{Image, Tone};

/// How large a picture is drawn. Each size keeps icons and image files in the
/// same slot, so rows with and without pictures keep one text spine.
#[derive(Clone, Copy)]
pub(super) enum PictureSize {
    /// Beside a row's title, an action or an accessory.
    Row,
    /// The subject of a grid cell.
    Cell,
}

/// Draws an image: a theme-tinted Lucide icon or an image file.
pub(super) fn picture(image: &Image, size: PictureSize, color: Hsla, theme: &Theme) -> AnyElement {
    match (image, size) {
        (Image::TintedIcon(name, tone), size) => picture(
            &Image::Icon(name.clone()),
            size,
            tone_color(*tone, color, theme),
            theme,
        ),
        (Image::Url(url), size) => img(SharedUri::from(url.clone()))
            .map(|image| match size {
                PictureSize::Row => image.size_4(),
                PictureSize::Cell => image.size_full(),
            })
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        (Image::FileIcon(path), size) => match file_icon(path) {
            Some(icon) => picture(&Image::File(icon), size, color, theme),
            None => picture(&Image::Icon(fallback_icon(path)), size, color, theme),
        },
        (Image::Circle(inner), size) => div()
            .map(|frame| match size {
                PictureSize::Row => frame.size_4(),
                PictureSize::Cell => frame.size_full(),
            })
            .rounded_full()
            .overflow_hidden()
            .child(match inner.as_ref() {
                // An image fills the circle; a cropped icon would lose its edges.
                Image::File(path) => img(path.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                Image::Url(url) => img(SharedUri::from(url.clone()))
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                other => picture(other, size, color, theme),
            })
            .into_any_element(),
        (Image::Icon(name), PictureSize::Row) => {
            icon(name).size_4().text_color(color).into_any_element()
        }
        (Image::Icon(name), PictureSize::Cell) => {
            icon(name).size_8().text_color(color).into_any_element()
        }
        (Image::File(path), PictureSize::Row) => img(path.clone())
            .size_4()
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        (Image::File(path), PictureSize::Cell) => img(path.clone())
            .size_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        (Image::Glyph(text), PictureSize::Row) => div()
            .size_4()
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .line_height(px(16.))
            .child(text.clone())
            .into_any_element(),
        (Image::Glyph(text), PictureSize::Cell) => div()
            .text_size(px(32.))
            .line_height(px(40.))
            .child(text.clone())
            .into_any_element(),
        (Image::Color(rgba), size) => div()
            .map(|swatch| match size {
                PictureSize::Row => swatch.size_4().rounded_sm(),
                PictureSize::Cell => swatch.size_full().rounded_md(),
            })
            .bg(gpui_kit::rgba(*rgba))
            .border_1()
            .border_color(color.opacity(0.2))
            .into_any_element(),
    }
}

/// The color a tone gives an icon; neutral keeps the surrounding color.
fn tone_color(tone: Tone, color: Hsla, theme: &Theme) -> Hsla {
    match tone {
        Tone::Neutral => color,
        Tone::Accent => theme.primary,
        Tone::Success => theme.success,
        Tone::Warning => theme.warning,
        Tone::Danger => theme.danger,
    }
}

thread_local! {
    /// Icons already looked up, so a row drawn every frame asks the system once.
    static FILE_ICONS: RefCell<HashMap<PathBuf, Option<PathBuf>>> = RefCell::default();
}

/// The system's icon for `path`, cached; the first lookup reads a PNG the
/// launcher keeps on disk once extracted.
fn file_icon(path: &Path) -> Option<PathBuf> {
    FILE_ICONS.with(|icons| {
        icons
            .borrow_mut()
            .entry(path.to_path_buf())
            .or_insert_with(|| crate::sources::applications::file_icon(path))
            .clone()
    })
}

/// A Lucide icon standing in where the system has no icon for a file.
fn fallback_icon(path: &Path) -> SharedString {
    match path.is_dir() {
        true => "folder".into(),
        false => "file".into(),
    }
}

fn icon(name: &SharedString) -> Icon {
    Icon::empty().path(format!("icons/{name}.svg"))
}

/// A short classification drawn in a tone. The theme decides what each tone
/// looks like, so a page can mark "Done" as success without naming a color.
pub(super) fn tag(text: SharedString, tone: Tone) -> AnyElement {
    match tone {
        Tone::Neutral => Tag::secondary(),
        Tone::Accent => Tag::info(),
        Tone::Success => Tag::success(),
        Tone::Warning => Tag::warning(),
        Tone::Danger => Tag::danger(),
    }
    .xsmall()
    .child(text)
    .into_any_element()
}
