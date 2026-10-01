//! Raster tiles: the parts of annotations drawn as pixels rather than paths.
//!
//! Text, step numbers and mosaic look exactly the same in the overlay and in
//! the exported image only if both show the same pixels, so they are drawn
//! here once, on the CPU, and the overlay displays the resulting tile.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};
use image::RgbaImage;
use tiny_skia::{FillRule, Mask, PathBuilder, Stroke, Transform};

use crate::{
    capture::Frame,
    geometry::PhysRect,
    scene::{Annotation, AnnotationId, Color, ScenePoint, Shape, mosaic_width},
};

/// Straight-alpha pixels placed at a whole-pixel desktop position.
#[derive(Clone, Debug)]
pub struct Tile {
    /// The desktop position of the tile's top-left pixel.
    x: i32,
    y: i32,
    image: Arc<RgbaImage>,
}

impl Tile {
    pub(crate) fn new(x: i32, y: i32, image: Arc<RgbaImage>) -> Self {
        Self { x, y, image }
    }

    pub fn x(&self) -> i32 {
        self.x
    }

    pub fn y(&self) -> i32 {
        self.y
    }

    pub fn image(&self) -> &Arc<RgbaImage> {
        &self.image
    }

    pub fn bounds(&self) -> PhysRect {
        PhysRect::new(
            self.x,
            self.y,
            self.image.width() as i32,
            self.image.height() as i32,
        )
    }
}

/// The tile of `annotation`, if it has one. `frame` is the display image a
/// mosaic pixelates.
pub fn tile(annotation: &Annotation, frame: Option<&Frame>) -> Option<Tile> {
    let style = annotation.style();
    match annotation.shape() {
        Shape::Text { origin, content } => {
            text_tile(content, *origin, style.font_size(), style.color())
        }
        Shape::Step { center, number } => {
            step_tile(*number, *center, style.font_size(), style.color())
        }
        Shape::Mosaic { points } => mosaic_tile(points, mosaic_width(style.stroke_width()), frame?),
        _ => None,
    }
}

/// Builds the tile of an annotation.
pub type TileBuilder = fn(&Annotation, Option<&Frame>) -> Option<Tile>;

/// Remembers each annotation's tile, rebuilt only when the annotation
/// changes: history snapshots keep unchanged annotations as the same `Arc`.
pub struct TileCache {
    tiles: HashMap<AnnotationId, (Arc<Annotation>, Option<Tile>)>,
    build: TileBuilder,
}

impl Default for TileCache {
    /// The tiles export draws over the figures: text, numbers, mosaic.
    fn default() -> Self {
        Self::new(tile)
    }
}

impl TileCache {
    pub fn new(build: TileBuilder) -> Self {
        Self {
            tiles: HashMap::new(),
            build,
        }
    }

    pub fn get(&mut self, annotation: &Arc<Annotation>, frame: Option<&Frame>) -> Option<Tile> {
        if let Some((known, tile)) = self.tiles.get(&annotation.id())
            && Arc::ptr_eq(known, annotation)
        {
            return tile.clone();
        }
        let tile = (self.build)(annotation, frame);
        self.tiles
            .insert(annotation.id(), (annotation.clone(), tile.clone()));
        tile
    }

    /// Forgets tiles of annotations no longer in `live`.
    pub fn retain(&mut self, live: &[Arc<Annotation>]) {
        self.tiles
            .retain(|id, _| live.iter().any(|annotation| annotation.id() == *id));
    }
}

struct Fonts {
    system: FontSystem,
    cache: SwashCache,
}

/// The shared font system. Loading the system's fonts takes a while, so
/// [`warm_up`] runs it in the background at startup.
fn fonts() -> &'static Mutex<Fonts> {
    static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
    FONTS.get_or_init(|| {
        Mutex::new(Fonts {
            system: FontSystem::new(),
            cache: SwashCache::new(),
        })
    })
}

pub fn warm_up() {
    std::thread::Builder::new()
        .name("snip-fonts".into())
        .spawn(|| {
            fonts();
        })
        .ok();
}

/// Lays out `text` and draws it into a fresh straight-alpha image, padded
/// so no glyph overhang is cut. Returns the image and the offset of the
/// layout's top-left inside it.
fn draw_text(text: &str, font_size: f32, color: Color, weight: Weight) -> Option<(RgbaImage, i32)> {
    let mut fonts = fonts().lock().ok()?;
    let Fonts { system, cache } = &mut *fonts;
    let line_height = (font_size * 1.3).ceil();
    let mut buffer = Buffer::new(system, Metrics::new(font_size, line_height));
    buffer.set_size(None, None);
    let attrs = Attrs::new().family(Family::SansSerif).weight(weight);
    buffer.set_text(text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(system, false);
    let (mut width, mut lines) = (0f32, 0usize);
    for run in buffer.layout_runs() {
        width = width.max(run.line_w);
        lines += 1;
    }
    let pad = (font_size * 0.25).ceil() as i32;
    let image_width = width.ceil() as i32 + pad * 2;
    let image_height = (lines.max(1) as f32 * line_height).ceil() as i32 + pad * 2;
    if image_width <= 0 || image_height <= 0 {
        return None;
    }
    let mut image = RgbaImage::new(image_width as u32, image_height as u32);
    let text_color = cosmic_text::Color::rgba(color.r, color.g, color.b, color.a);
    buffer.draw(system, cache, text_color, |x, y, w, h, glyph| {
        for py in y..y + h as i32 {
            for px in x..x + w as i32 {
                let (ix, iy) = (px + pad, py + pad);
                if ix < 0 || iy < 0 || ix >= image_width || iy >= image_height {
                    continue;
                }
                blend(image.get_pixel_mut(ix as u32, iy as u32), glyph);
            }
        }
    });
    Some((image, pad))
}

/// Source-over of a straight-alpha glyph pixel onto a straight-alpha pixel.
fn blend(pixel: &mut image::Rgba<u8>, source: cosmic_text::Color) {
    let source_alpha = source.a() as f32 / 255.;
    if source_alpha <= 0. {
        return;
    }
    let [r, g, b, a] = pixel.0;
    let destination_alpha = a as f32 / 255.;
    let out_alpha = source_alpha + destination_alpha * (1. - source_alpha);
    let mix = |source: u8, destination: u8| {
        ((source as f32 * source_alpha
            + destination as f32 * destination_alpha * (1. - source_alpha))
            / out_alpha)
            .round() as u8
    };
    pixel.0 = [
        mix(source.r(), r),
        mix(source.g(), g),
        mix(source.b(), b),
        (out_alpha * 255.).round() as u8,
    ];
}

fn text_tile(content: &str, origin: ScenePoint, font_size: f32, color: Color) -> Option<Tile> {
    let (image, pad) = draw_text(content, font_size, color, Weight::NORMAL)?;
    Some(Tile {
        x: origin.x.round() as i32 - pad,
        y: origin.y.round() as i32 - pad,
        image: Arc::new(image),
    })
}

/// Text as an image to pin: dark type on a light card with a margin. The
/// card is content the user keeps, so its colors are fixed, not themed.
pub fn text_card(text: &str) -> Option<RgbaImage> {
    const MARGIN: u32 = 16;
    let (ink, _) = draw_text(
        text.trim_end(),
        16.,
        Color::rgb(0x1F, 0x23, 0x28),
        Weight::NORMAL,
    )?;
    let mut card = RgbaImage::from_pixel(
        ink.width() + MARGIN * 2,
        ink.height() + MARGIN * 2,
        image::Rgba([0xFF, 0xFF, 0xFF, 0xFF]),
    );
    image::imageops::overlay(&mut card, &ink, MARGIN as i64, MARGIN as i64);
    Some(card)
}

/// The number of a step badge, centred on the badge. The badge itself is a
/// filled circle figure under it.
fn step_tile(number: u32, center: ScenePoint, font_size: f32, color: Color) -> Option<Tile> {
    let digits_size = font_size * if number >= 10 { 0.8 } else { 0.95 };
    let (image, _) = draw_text(
        &number.to_string(),
        digits_size,
        color.contrasting(),
        Weight::BOLD,
    )?;
    // Centre the ink, not the line box: digits sit above the baseline.
    let ink = ink_bounds(&image)?;
    let ink_center_x = ink.x as f32 + ink.width as f32 / 2.;
    let ink_center_y = ink.y as f32 + ink.height as f32 / 2.;
    Some(Tile {
        x: (center.x - ink_center_x).round() as i32,
        y: (center.y - ink_center_y).round() as i32,
        image: Arc::new(image),
    })
}

/// The rectangle of pixels that aren't fully transparent.
fn ink_bounds(image: &RgbaImage) -> Option<PhysRect> {
    let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel.0[3] > 0 {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
        }
    }
    (left < right)
        .then(|| PhysRect::from_edges(left as i32, top as i32, right as i32, bottom as i32))
}

/// The block size of a mosaic brush of `width` pixels.
pub fn mosaic_block(width: f32) -> i32 {
    ((width / 2.).round() as i32).clamp(6, 32)
}

/// Pixelates `frame` under a brush stroke: each block, aligned to a grid on
/// the desktop so overlapping strokes agree, takes the average of the
/// pixels it covers, and the brush's coverage becomes the tile's alpha.
fn mosaic_tile(points: &[ScenePoint], width: f32, frame: &Frame) -> Option<Tile> {
    let radius = width / 2.;
    let (min_x, min_y, max_x, max_y) = points.iter().fold(
        (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
        |(min_x, min_y, max_x, max_y), point| {
            (
                min_x.min(point.x),
                min_y.min(point.y),
                max_x.max(point.x),
                max_y.max(point.y),
            )
        },
    );
    let block = mosaic_block(width);
    let snap_down = |value: f32| ((value - radius).floor() as i32).div_euclid(block) * block;
    let snap_up =
        |value: f32| ((value + radius).ceil() as i32 + block - 1).div_euclid(block) * block;
    let area = PhysRect::from_edges(
        snap_down(min_x),
        snap_down(min_y),
        snap_up(max_x),
        snap_up(max_y),
    )
    .intersect(&frame.bounds())?;

    let mut path = PathBuilder::new();
    let (first, rest) = points.split_first()?;
    path.move_to(first.x - area.x as f32, first.y - area.y as f32);
    if rest.is_empty() {
        path.line_to(first.x - area.x as f32 + 0.01, first.y - area.y as f32);
    }
    for point in rest {
        path.line_to(point.x - area.x as f32, point.y - area.y as f32);
    }
    let stroke = Stroke {
        width,
        line_cap: tiny_skia::LineCap::Round,
        line_join: tiny_skia::LineJoin::Round,
        ..Stroke::default()
    };
    let outline = path.finish()?.stroke(&stroke, 1.)?;
    let mut mask = Mask::new(area.width as u32, area.height as u32)?;
    mask.fill_path(&outline, FillRule::Winding, true, Transform::identity());

    let mut image = RgbaImage::new(area.width as u32, area.height as u32);
    let bounds = frame.bounds();
    let pixels = frame.pixels();
    let mut block_y = area.y.div_euclid(block) * block;
    while block_y < area.bottom() {
        let mut block_x = area.x.div_euclid(block) * block;
        while block_x < area.right() {
            let cell = PhysRect::new(block_x, block_y, block, block).intersect(&area);
            if let Some(cell) = cell {
                let average = average(pixels, &bounds, &cell);
                for y in cell.y..cell.bottom() {
                    for x in cell.x..cell.right() {
                        let (ix, iy) = ((x - area.x) as u32, (y - area.y) as u32);
                        let coverage = mask.data()[(iy * area.width as u32 + ix) as usize];
                        if coverage > 0 {
                            image.put_pixel(
                                ix,
                                iy,
                                image::Rgba([average.r, average.g, average.b, coverage]),
                            );
                        }
                    }
                }
            }
            block_x += block;
        }
        block_y += block;
    }
    Some(Tile {
        x: area.x,
        y: area.y,
        image: Arc::new(image),
    })
}

/// The mean color of `cell`, a desktop rectangle inside the frame `bounds`.
fn average(pixels: &[u8], bounds: &PhysRect, cell: &PhysRect) -> Color {
    let (mut r, mut g, mut b, mut count) = (0u64, 0u64, 0u64, 0u64);
    for y in cell.y..cell.bottom() {
        let row = (y - bounds.y) as usize * bounds.width as usize;
        for x in cell.x..cell.right() {
            let offset = (row + (x - bounds.x) as usize) * 4;
            r += pixels[offset] as u64;
            g += pixels[offset + 1] as u64;
            b += pixels[offset + 2] as u64;
            count += 1;
        }
    }
    let count = count.max(1);
    Color::rgb((r / count) as u8, (g / count) as u8, (b / count) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        geometry::DisplayArea,
        scene::{Style, Tool},
    };

    /// Left half black, right half white.
    fn halves(bounds: PhysRect) -> Frame {
        let mut pixels = Vec::new();
        for _ in 0..bounds.height {
            for x in 0..bounds.width {
                let value = if x < bounds.width / 2 { 0 } else { 255 };
                pixels.extend_from_slice(&[value, value, value, 255]);
            }
        }
        Frame::new(DisplayArea::new(bounds, 1.), 0, pixels).unwrap()
    }

    #[test]
    fn test_mosaic_blocks_average_and_align() {
        let frame = halves(PhysRect::new(0, 0, 64, 64));
        let points = [ScenePoint::new(10., 32.), ScenePoint::new(54., 32.)];
        let tile = mosaic_tile(&points, 16., &frame).unwrap();
        let block = mosaic_block(16.);
        assert_eq!(tile.x() % block, 0, "blocks align to the desktop grid");
        // Every opaque pixel of one block has one color.
        let image = tile.image();
        let probe = |x: i32, y: i32| {
            image
                .get_pixel((x - tile.x()) as u32, (y - tile.y()) as u32)
                .0
        };
        assert_eq!(probe(9, 32), probe(10, 33));
        assert_eq!(probe(9, 32)[0], 0, "left half stays dark");
        assert_eq!(probe(55, 32)[0], 255, "right half stays light");
    }

    #[test]
    fn test_mosaic_off_display_has_no_tile() {
        let frame = halves(PhysRect::new(0, 0, 32, 32));
        assert!(mosaic_tile(&[ScenePoint::new(500., 500.)], 16., &frame).is_none());
    }

    #[test]
    fn test_cache_rebuilds_only_changed_annotations() {
        let frame = halves(PhysRect::new(0, 0, 64, 64));
        let shape = Shape::start(Tool::Mosaic, ScenePoint::new(20., 20.)).unwrap();
        let annotation = Arc::new(Annotation::new(AnnotationId(1), shape, Style::default()));
        let mut cache = TileCache::default();
        let first = cache.get(&annotation, Some(&frame)).unwrap();
        let again = cache.get(&annotation, Some(&frame)).unwrap();
        assert!(Arc::ptr_eq(first.image(), again.image()));

        let moved = Arc::new(annotation.with_shape(annotation.shape().translate(8., 0.)));
        let rebuilt = cache.get(&moved, Some(&frame)).unwrap();
        assert!(!Arc::ptr_eq(first.image(), rebuilt.image()));

        cache.retain(&[]);
        assert!(cache.tiles.is_empty());
    }

    #[test]
    fn test_ink_bounds() {
        let mut image = RgbaImage::new(10, 10);
        image.put_pixel(2, 3, image::Rgba([0, 0, 0, 255]));
        image.put_pixel(5, 7, image::Rgba([0, 0, 0, 10]));
        assert_eq!(ink_bounds(&image), Some(PhysRect::new(2, 3, 4, 5)));
        assert_eq!(ink_bounds(&RgbaImage::new(2, 2)), None);
    }
}
