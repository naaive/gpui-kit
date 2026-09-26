//! The small visual vocabulary every region shares: pictures and tags.

use gpui_kit::{
    AnyElement, Hsla, IntoElement, ObjectFit, ParentElement as _, SharedString, Styled as _,
    StyledImage as _,
    component::{Icon, Sizable as _, tag::Tag},
    img,
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
