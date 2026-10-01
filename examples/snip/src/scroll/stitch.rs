//! Joining successive captures of a scrolling area into one tall image.
//!
//! Rows are compared by hash. Rows that are the same at the same place in
//! two captures are a fixed header or footer (a toolbar, a status bar), kept
//! once; between them, the scroll is the offset at which the most rows of
//! the new capture repeat rows of the last one.

use std::hash::{DefaultHasher, Hash as _, Hasher as _};

use image::RgbaImage;

/// The share of overlapping rows that must agree for an offset to count;
/// the rest may differ (a blinking caret, an animation).
const MIN_AGREEMENT: f32 = 0.9;
/// Fewer overlapping rows than this can't tell one offset from another.
const MIN_OVERLAP: usize = 8;

/// What a new capture added.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    /// This many new rows were appended.
    Added(usize),
    /// The capture is the same as the last: the end was reached.
    Unchanged,
    /// The capture doesn't continue the last one (it scrolled too far, or
    /// something else changed).
    Lost,
}

pub struct Stitcher {
    width: u32,
    height: usize,
    /// Rows at the top and bottom that stay put while the content scrolls,
    /// known once the first scroll is seen.
    fixed: Option<(usize, usize)>,
    first: RgbaImage,
    last: RgbaImage,
    last_hashes: Vec<u64>,
    /// The scrolled rows appended after the first capture's, as RGBA.
    appended: Vec<u8>,
    appended_rows: usize,
}

impl Stitcher {
    pub fn new(first: RgbaImage) -> Self {
        let last_hashes = row_hashes(&first);
        Self {
            width: first.width(),
            height: first.height() as usize,
            fixed: None,
            last: first.clone(),
            first,
            last_hashes,
            appended: Vec::new(),
            appended_rows: 0,
        }
    }

    /// The height of the joined image so far.
    pub fn height(&self) -> usize {
        self.height + self.appended_rows
    }

    pub fn push(&mut self, next: RgbaImage) -> Step {
        if next.dimensions() != self.last.dimensions() {
            return Step::Lost;
        }
        let hashes = row_hashes(&next);
        if hashes == self.last_hashes {
            return Step::Unchanged;
        }
        let (top, bottom) = *self
            .fixed
            .get_or_insert_with(|| fixed_rows(&self.last_hashes, &hashes));
        let middle = top..self.height - bottom;
        if middle.len() <= MIN_OVERLAP {
            return Step::Lost;
        }
        let Some(offset) =
            scroll_offset(&self.last_hashes[middle.clone()], &hashes[middle.clone()])
        else {
            return Step::Lost;
        };
        // The new capture's last `offset` scrolling rows are new.
        let row_bytes = self.width as usize * 4;
        let start = (middle.end - offset) * row_bytes;
        self.appended
            .extend_from_slice(&next.as_raw()[start..middle.end * row_bytes]);
        self.appended_rows += offset;
        self.last = next;
        self.last_hashes = hashes;
        Step::Added(offset)
    }

    /// The joined image: the first capture down to its footer, every
    /// appended row, then the footer as last seen.
    pub fn finish(self) -> RgbaImage {
        let (_, bottom) = self.fixed.unwrap_or((0, 0));
        let row_bytes = self.width as usize * 4;
        let body_end = (self.height - bottom) * row_bytes;
        let mut pixels = Vec::with_capacity(self.height() * row_bytes);
        pixels.extend_from_slice(&self.first.as_raw()[..body_end]);
        pixels.extend_from_slice(&self.appended);
        pixels.extend_from_slice(&self.last.as_raw()[body_end..]);
        RgbaImage::from_raw(self.width, self.height() as u32, pixels)
            .expect("the rows add up to the image")
    }
}

/// One hash per row. The right edge is left out: a scrollbar sits there and
/// moves with every scroll, which would make every row differ.
fn row_hashes(image: &RgbaImage) -> Vec<u64> {
    let width = image.width() as usize;
    let compared = width - (width / 20).min(40);
    image
        .as_raw()
        .chunks_exact(width * 4)
        .map(|row| {
            let mut hasher = DefaultHasher::new();
            row[..compared * 4].hash(&mut hasher);
            hasher.finish()
        })
        .collect()
}

/// How many rows at the top and at the bottom are the same in both.
fn fixed_rows(before: &[u64], after: &[u64]) -> (usize, usize) {
    let top = before.iter().zip(after).take_while(|(a, b)| a == b).count();
    let bottom = before
        .iter()
        .rev()
        .zip(after.iter().rev())
        .take(before.len() - top)
        .take_while(|(a, b)| a == b)
        .count();
    (top, bottom)
}

/// How far `after` is scrolled down from `before`: the offset at which the
/// most of `after`'s rows repeat `before`'s, if enough of them do.
///
/// Only rows that differ from the row above them are compared: a run of
/// blank rows repeats at every offset and would make a wrong one look as
/// good as the right one.
fn scroll_offset(before: &[u64], after: &[u64]) -> Option<usize> {
    let rows = before.len();
    let edges: Vec<usize> = (1..rows).filter(|ix| after[*ix] != after[ix - 1]).collect();
    let mut best: Option<(usize, f32)> = None;
    for offset in 1..rows.saturating_sub(MIN_OVERLAP) {
        let overlap = rows - offset;
        let compared: Vec<usize> = edges.iter().copied().filter(|ix| *ix < overlap).collect();
        if compared.len() < MIN_OVERLAP / 2 {
            continue;
        }
        let agreeing = compared
            .iter()
            .filter(|ix| before[*ix + offset] == after[**ix])
            .count();
        let agreement = agreeing as f32 / compared.len() as f32;
        if agreement >= MIN_AGREEMENT && best.is_none_or(|(_, score)| agreement > score) {
            best = Some((offset, agreement));
        }
    }
    best.map(|(offset, _)| offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page of `rows` rows, each a distinct shade, seen through a window
    /// `height` rows tall scrolled down by `scroll`, under a fixed header.
    fn view(scroll: u32, height: u32, header: u32) -> RgbaImage {
        RgbaImage::from_fn(60, height, |x, y| {
            if y < header {
                return image::Rgba([0, 0, 255, 255]);
            }
            let row = y - header + scroll;
            image::Rgba([(row % 251) as u8, (row / 251) as u8, (x % 7) as u8, 255])
        })
    }

    #[test]
    fn test_stitches_scrolled_views_under_a_fixed_header() {
        let mut stitcher = Stitcher::new(view(0, 100, 10));
        assert_eq!(stitcher.push(view(30, 100, 10)), Step::Added(30));
        assert_eq!(stitcher.push(view(55, 100, 10)), Step::Added(25));
        assert_eq!(stitcher.push(view(55, 100, 10)), Step::Unchanged);
        let image = stitcher.finish();
        assert_eq!(image.height(), 100 + 55);
        assert_eq!(
            image,
            view(0, 155, 10),
            "the page from the top, header first"
        );
        assert_eq!(
            image.get_pixel(0, 0).0,
            [0, 0, 255, 255],
            "the header is kept once, at the top"
        );
    }

    #[test]
    fn test_blank_stretches_dont_fool_the_offset() {
        // Text lines every 20 rows on white, like a document.
        let page = |scroll: u32| {
            RgbaImage::from_fn(60, 120, |x, y| {
                let row = y + scroll;
                if row % 20 < 3 {
                    image::Rgba([(row / 20 % 251) as u8, 0, (x % 5) as u8, 255])
                } else {
                    image::Rgba([255, 255, 255, 255])
                }
            })
        };
        let mut stitcher = Stitcher::new(page(0));
        assert_eq!(stitcher.push(page(37)), Step::Added(37));
    }

    #[test]
    fn test_a_jump_past_the_view_is_lost() {
        let mut stitcher = Stitcher::new(view(0, 100, 0));
        assert_eq!(stitcher.push(view(500, 100, 0)), Step::Lost);
        assert_eq!(stitcher.height(), 100);
    }

    #[test]
    fn test_a_moving_scrollbar_is_ignored() {
        let with_thumb = |scroll: u32| {
            let mut image = view(scroll, 100, 0);
            for y in scroll / 2..scroll / 2 + 20 {
                for x in 57..60 {
                    image.put_pixel(x, y, image::Rgba([90, 90, 90, 255]));
                }
            }
            image
        };
        let mut stitcher = Stitcher::new(with_thumb(0));
        assert_eq!(stitcher.push(with_thumb(40)), Step::Added(40));
    }
}
