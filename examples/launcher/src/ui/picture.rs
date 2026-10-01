//! The small visual vocabulary every region shares: pictures and tags.

use gpui_kit::{
    AnyElement, Hsla, IntoElement, ObjectFit, ParentElement as _, SharedString, Styled as _,
    StyledImage as _,
    component::{Icon, Sizable as _, tag::Tag},
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
pub(super) fn picture(image: &Image, size: PictureSize, color: Hsla) -> AnyElement {
    match (image, size) {
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
