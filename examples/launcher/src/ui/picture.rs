//! The small visual vocabulary every region shares: pictures and tags.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
};

use gpui_kit::{
    AnyElement, Hsla, IntoElement, ObjectFit, ParentElement as _, SharedString, SharedUri,
    Styled as _, StyledImage as _,
    component::{Icon, Theme},
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

/// The side of a row's picture square.
const BADGE_SIZE: f32 = 22.;

/// The colors a command's square is painted in, chosen by its icon so each
/// command keeps its color wherever it is listed.
const BADGE_COLORS: [u32; 9] = [
    0xc98a2e, 0x6b5bd6, 0x4f7fd9, 0x7b63d9, 0x3a9a62, 0xc8574c, 0xc25689, 0x5e5e66, 0x2f8f9d,
];

/// How a row's picture sits in its square.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Badge {
    /// A white icon on a color of its own, as the root search lists commands.
    Colored,
    /// A quiet icon on a faint square, for the lists inside a command.
    Neutral,
}

/// A row's picture in a fixed square: an icon on a rounded square, an image
/// file or application icon filling it, so every row keeps one spine.
pub(crate) fn badge(image: &Image, style: Badge, muted: Hsla, theme: &Theme) -> AnyElement {
    let square = || {
        div()
            .flex_none()
            .size(px(BADGE_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
    };
    let icon_square = |name: &SharedString, tone: Option<Tone>| {
        let (background, color) = match (style, tone) {
            (Badge::Colored, Some(tone)) if tone != Tone::Neutral => {
                (tone_color(tone, muted, theme), gpui_kit::white())
            }
            (Badge::Colored, _) => (badge_color(name), gpui_kit::white()),
            (Badge::Neutral, tone) => (
                theme.foreground.opacity(0.07),
                tone.map_or(muted, |tone| tone_color(tone, muted, theme)),
            ),
        };
        square()
            .bg(background)
            .child(icon(name).size(px(13.)).text_color(color))
            .into_any_element()
    };
    match image {
        Image::Icon(name) => icon_square(name, None),
        Image::TintedIcon(name, tone) => icon_square(name, Some(*tone)),
        Image::Glyph(text) => square()
            .when(style == Badge::Neutral, |this| {
                this.bg(theme.foreground.opacity(0.07))
            })
            .text_size(px(15.))
            .line_height(px(BADGE_SIZE))
            .child(text.clone())
            .into_any_element(),
        Image::Color(rgba) => square()
            .bg(gpui_kit::rgba(*rgba))
            .border_1()
            .border_color(theme.foreground.opacity(0.1))
            .into_any_element(),
        Image::File(path) => img(path.clone())
            .size(px(BADGE_SIZE))
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        Image::Url(url) => img(SharedUri::from(url.clone()))
            .size(px(BADGE_SIZE))
            .rounded(px(6.))
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        Image::FileIcon(path) => match file_icon(path) {
            Some(icon) => badge(&Image::File(icon), style, muted, theme),
            None => badge(
                &Image::Icon(fallback_icon(path)),
                Badge::Neutral,
                muted,
                theme,
            ),
        },
        Image::Circle(_) => square()
            .child(picture(image, PictureSize::Cell, muted, theme))
            .into_any_element(),
    }
}

/// The color of a command's square, from its icon's name.
fn badge_color(name: &str) -> Hsla {
    // FNV-1a: stable across runs and platforms, unlike the std hasher.
    let hash = name.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    gpui_kit::rgb(BADGE_COLORS[hash as usize % BADGE_COLORS.len()]).into()
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

/// A short classification drawn in a tone: quiet text on a faint chip of
/// its color. The theme decides what each tone looks like, so a page can
/// mark "Done" as success without naming a color.
pub(super) fn tag(text: SharedString, tone: Tone, theme: &Theme) -> AnyElement {
    let color = match tone {
        Tone::Neutral => theme.muted_foreground,
        tone => tone_color(tone, theme.muted_foreground, theme),
    };
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(4.))
        .bg(match tone {
            Tone::Neutral => theme.foreground.opacity(0.07),
            _ => color.opacity(0.14),
        })
        .text_color(color)
        .text_size(px(11.))
        .child(text)
        .into_any_element()
}
