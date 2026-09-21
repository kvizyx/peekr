//! CPU rasterizer for the triangle meshes egui produces.
//!
//! egui tessellates everything into textured, vertex-colored triangles with premultiplied alpha
//! and anti-aliases edges itself (feathering), so a plain scanline rasterizer that blends in
//! gamma space draws it the same way the GPU backends do.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::ops::Range;

use egui::epaint::textures::{TextureFilter, TexturesDelta};
use egui::epaint::{ClippedPrimitive, ImageData, ImageDelta, Mesh, Primitive, Vertex};
use egui::{Color32, Rect, TextureId};

/// Areas smaller than this are drawn on the calling thread; spawning threads would cost more.
const PARALLEL_MIN_PIXELS: usize = 256 * 256;
const MAX_THREADS: usize = 8;

/// The part of a frame that was drawn again, as ranges of rows and of columns within them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Damage {
    pub rows: Range<usize>,
    pub columns: Range<usize>,
}

/// Pixels in the format softbuffer presents: `0x00RRGGBB`, row by row.
pub struct Frame<'a> {
    pub pixels: &'a mut [u32],
    pub width: usize,
    pub height: usize,
}

/// Rows of a frame drawn by one thread.
struct Band<'a> {
    pixels: &'a mut [u32],
    width: usize,
    /// Frame row of the first row in `pixels`.
    top: usize,
}

#[derive(Default)]
pub struct Renderer {
    textures: HashMap<TextureId, Texture>,
    /// Last frame's primitives, to work out which pixels changed.
    previous: Vec<ClippedPrimitive>,
    /// Size of the frame the previous primitives were drawn into.
    size: (usize, usize),
}

struct Texture {
    width: usize,
    height: usize,
    pixels: Vec<Color32>,
    filter: TextureFilter,
    /// Whether every texel is opaque, so drawing the texture hides what is under it.
    opaque: bool,
}

/// Pixel area triangles are cut to; `max` is exclusive.
#[derive(Clone, Copy)]
struct Clip {
    min_x: i32,
    min_y: i32,
    max_x: i32,
    max_y: i32,
}

/// A vertex in pixels with the values interpolated across the triangle: premultiplied RGBA
/// (0–255) and texture coordinates.
#[derive(Clone, Copy)]
struct Point {
    x: f32,
    y: f32,
    values: [f32; 6],
}

impl Renderer {
    /// Draws the primitives, applying (and consuming) texture changes, and returns the part of
    /// the frame that changed. Only those pixels are drawn again, so the frame has to keep its
    /// contents in between.
    pub fn render(
        &mut self,
        frame: &mut Frame<'_>,
        primitives: &[ClippedPrimitive],
        textures: &mut TexturesDelta,
        pixels_per_point: f32,
    ) -> Option<Damage> {
        let textures_changed = !textures.set.is_empty() || !textures.free.is_empty();

        for (id, deltas) in textures.set.drain() {
            for delta in deltas {
                self.set_texture(id, &delta);
            }
        }

        let mut drawn = None;

        if frame.width > 0 {
            // A texture can be drawn anywhere, so its change is not worth tracking in detail.
            let damage = if textures_changed || self.size != (frame.width, frame.height) {
                Clip::frame(frame)
            } else {
                self.damage(primitives, frame, pixels_per_point)
            };

            if !damage.is_empty() {
                self.draw(frame, primitives, pixels_per_point, damage);
                drawn = Some(Damage {
                    rows: damage.min_y as usize..damage.max_y as usize,
                    columns: damage.min_x as usize..damage.max_x as usize,
                });
            }

            self.previous.clear();
            self.previous.extend_from_slice(primitives);
            self.size = (frame.width, frame.height);
        }

        for id in textures.free.drain() {
            self.textures.remove(&id);
        }

        drawn
    }

    /// The pixels where this frame differs from the last one.
    fn damage(&self, primitives: &[ClippedPrimitive], frame: &Frame<'_>, pixels_per_point: f32) -> Clip {
        if self.previous.len() != primitives.len() {
            return Clip::frame(frame);
        }

        let mut damage = Clip::EMPTY;
        for (previous, current) in self.previous.iter().zip(primitives) {
            damage = damage.union(changed(previous, current, frame, pixels_per_point));
        }

        damage
    }

    /// Whether the primitive paints over the whole damaged area without blending, hiding both
    /// the cleared background and everything drawn before it.
    #[expect(clippy::float_cmp, reason = "the bounds are the vertex positions themselves")]
    fn covers(&self, primitive: &ClippedPrimitive, damage: Clip, pixels_per_point: f32) -> bool {
        let Primitive::Mesh(mesh) = &primitive.primitive else {
            return false;
        };

        // Two triangles of a rectangle, with no transparency anywhere.
        if mesh.indices.len() != 6 || mesh.vertices.len() != 4 {
            return false;
        }
        if mesh.vertices.iter().any(|vertex| vertex.color.a() != 255) {
            return false;
        }
        if !self
            .textures
            .get(&mesh.texture_id)
            .is_some_and(|texture| texture.opaque)
        {
            return false;
        }

        let bounds = mesh.calc_bounds();
        let is_corner = |vertex: &Vertex| {
            (vertex.pos.x == bounds.min.x || vertex.pos.x == bounds.max.x)
                && (vertex.pos.y == bounds.min.y || vertex.pos.y == bounds.max.y)
        };

        mesh.vertices.iter().all(is_corner)
            && Clip::inside(bounds.intersect(primitive.clip_rect), pixels_per_point).contains(damage)
    }

    /// Splits the damaged rows into horizontal bands drawn on separate threads.
    fn draw(&self, frame: &mut Frame<'_>, primitives: &[ClippedPrimitive], pixels_per_point: f32, damage: Clip) {
        let covered = primitives
            .iter()
            .rposition(|primitive| self.covers(primitive, damage, pixels_per_point));
        let primitives = &primitives[covered.unwrap_or(0)..];
        let clear = covered.is_none();

        let (width, top) = (frame.width, damage.min_y as usize);
        let rows = damage.max_y as usize - top;
        let damaged_pixels = rows * (damage.max_x - damage.min_x) as usize;
        let threads = if damaged_pixels < PARALLEL_MIN_PIXELS {
            1
        } else {
            std::thread::available_parallelism()
                .map_or(1, NonZeroUsize::get)
                .min(MAX_THREADS)
        };
        let band_rows = rows.div_ceil(threads).max(1);
        let damaged = &mut frame.pixels[top * width..damage.max_y as usize * width];

        std::thread::scope(|scope| {
            for (index, pixels) in damaged.chunks_mut(band_rows * width).enumerate() {
                let mut band = Band {
                    pixels,
                    width,
                    top: top + index * band_rows,
                };

                if threads == 1 {
                    self.draw_band(&mut band, primitives, pixels_per_point, damage, clear);
                } else {
                    scope.spawn(move || self.draw_band(&mut band, primitives, pixels_per_point, damage, clear));
                }
            }
        });
    }

    fn draw_band(
        &self,
        band: &mut Band<'_>,
        primitives: &[ClippedPrimitive],
        pixels_per_point: f32,
        damage: Clip,
        clear: bool,
    ) {
        if clear {
            let columns = damage.min_x as usize..damage.max_x as usize;

            for row in band.pixels.chunks_mut(band.width) {
                row[columns.clone()].fill(0);
            }
        }

        for primitive in primitives {
            let Primitive::Mesh(mesh) = &primitive.primitive else {
                continue;
            };

            let clip = Clip::new(primitive.clip_rect, pixels_per_point, band).intersect(damage);
            if !clip.is_empty() {
                self.draw_mesh(band, mesh, clip, pixels_per_point);
            }
        }
    }

    fn set_texture(&mut self, id: TextureId, delta: &ImageDelta) {
        let ImageData::Color(image) = &delta.image;
        let [width, height] = image.size;

        let opaque = image.pixels.iter().all(|pixel| pixel.a() == 255);

        let Some([x, y]) = delta.pos else {
            let texture = Texture {
                width,
                height,
                pixels: image.pixels.clone(),
                filter: delta.options.magnification,
                opaque,
            };
            self.textures.insert(id, texture);
            return;
        };

        let Some(texture) = self.textures.get_mut(&id) else {
            return;
        };
        texture.opaque &= opaque;

        for (row, source) in image.pixels.chunks_exact(width).enumerate() {
            let start = (y + row) * texture.width + x;
            texture.pixels[start..start + width].copy_from_slice(source);
        }
    }

    fn draw_mesh(&self, band: &mut Band<'_>, mesh: &Mesh, clip: Clip, pixels_per_point: f32) {
        let Some(texture) = self.textures.get(&mesh.texture_id) else {
            return;
        };

        for triangle in mesh.indices.as_chunks::<3>().0 {
            let vertex = |index: u32| mesh.vertices.get(index as usize);
            let (Some(a), Some(b), Some(c)) = (vertex(triangle[0]), vertex(triangle[1]), vertex(triangle[2])) else {
                continue;
            };

            let points = [a, b, c].map(|v| Point::new(v, pixels_per_point));
            draw_triangle(band, texture, clip, points);
        }
    }
}

impl Clip {
    const EMPTY: Self = Self {
        min_x: 0,
        min_y: 0,
        max_x: 0,
        max_y: 0,
    };

    fn frame(frame: &Frame<'_>) -> Self {
        Self {
            min_x: 0,
            min_y: 0,
            max_x: frame.width as i32,
            max_y: frame.height as i32,
        }
    }

    /// The pixels a rectangle covers completely, with its edges rounded inwards.
    fn inside(rect: Rect, pixels_per_point: f32) -> Self {
        Self {
            min_x: (rect.min.x * pixels_per_point).ceil() as i32,
            min_y: (rect.min.y * pixels_per_point).ceil() as i32,
            max_x: (rect.max.x * pixels_per_point).floor() as i32,
            max_y: (rect.max.y * pixels_per_point).floor() as i32,
        }
    }

    /// The pixels a rectangle can touch, with one to spare for rounding and anti-aliasing.
    fn around(rect: Rect, pixels_per_point: f32, frame: &Frame<'_>) -> Self {
        let clamp = |value: f32, max: usize| (value as i32).clamp(0, max as i32);

        Self {
            min_x: clamp((rect.min.x * pixels_per_point).floor() - 1.0, frame.width),
            min_y: clamp((rect.min.y * pixels_per_point).floor() - 1.0, frame.height),
            max_x: clamp((rect.max.x * pixels_per_point).ceil() + 1.0, frame.width),
            max_y: clamp((rect.max.y * pixels_per_point).ceil() + 1.0, frame.height),
        }
    }

    fn is_empty(self) -> bool {
        self.min_x >= self.max_x || self.min_y >= self.max_y
    }

    fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }

        Self {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    fn intersect(self, other: Self) -> Self {
        Self {
            min_x: self.min_x.max(other.min_x),
            min_y: self.min_y.max(other.min_y),
            max_x: self.max_x.min(other.max_x),
            max_y: self.max_y.min(other.max_y),
        }
    }

    fn contains(self, other: Self) -> bool {
        other.is_empty()
            || (self.min_x <= other.min_x
                && self.min_y <= other.min_y
                && self.max_x >= other.max_x
                && self.max_y >= other.max_y)
    }

    fn new(rect: Rect, pixels_per_point: f32, band: &Band<'_>) -> Self {
        let (top, bottom) = (band.top as f32, (band.top + band.pixels.len() / band.width) as f32);
        let to_pixels = |value: f32, min: f32, max: f32| (value * pixels_per_point).round().clamp(min, max) as i32;

        Self {
            min_x: to_pixels(rect.min.x, 0.0, band.width as f32),
            min_y: to_pixels(rect.min.y, top, bottom),
            max_x: to_pixels(rect.max.x, 0.0, band.width as f32),
            max_y: to_pixels(rect.max.y, top, bottom),
        }
    }
}

impl Point {
    fn new(vertex: &Vertex, pixels_per_point: f32) -> Self {
        let [r, g, b, a] = vertex.color.to_array().map(f32::from);

        Self {
            x: vertex.pos.x * pixels_per_point,
            y: vertex.pos.y * pixels_per_point,
            values: [r, g, b, a, vertex.uv.x, vertex.uv.y],
        }
    }
}

/// The pixels where a primitive differs from the way it was drawn last frame.
///
/// egui merges everything it can into one mesh, so a mesh that spans the screen usually has only
/// a few triangles that moved; the rest of it is left alone.
fn changed(previous: &ClippedPrimitive, current: &ClippedPrimitive, frame: &Frame<'_>, pixels_per_point: f32) -> Clip {
    let whole = || bounds(previous, frame, pixels_per_point).union(bounds(current, frame, pixels_per_point));

    let (Primitive::Mesh(previous_mesh), Primitive::Mesh(current_mesh)) = (&previous.primitive, &current.primitive)
    else {
        return whole();
    };
    if previous.clip_rect != current.clip_rect
        || previous_mesh.texture_id != current_mesh.texture_id
        || previous_mesh.indices != current_mesh.indices
        || previous_mesh.vertices.len() != current_mesh.vertices.len()
    {
        return whole();
    }
    if previous_mesh.vertices == current_mesh.vertices {
        return Clip::EMPTY;
    }

    let clip = previous.clip_rect;
    let mut damage = Clip::EMPTY;

    for triangle in previous_mesh.indices.as_chunks::<3>().0 {
        let vertices = |mesh: &Mesh| triangle.map(|index| mesh.vertices.get(index as usize).copied());
        let (previous_vertices, current_vertices) = (vertices(previous_mesh), vertices(current_mesh));

        if previous_vertices == current_vertices {
            continue;
        }

        for corners in [previous_vertices, current_vertices] {
            let rect = corners.iter().flatten().fold(Rect::NOTHING, |rect, vertex| {
                rect.union(Rect::from_min_max(vertex.pos, vertex.pos))
            });

            damage = damage.union(Clip::around(rect.intersect(clip), pixels_per_point, frame));
        }
    }

    damage
}

/// The pixels a primitive can touch.
fn bounds(primitive: &ClippedPrimitive, frame: &Frame<'_>, pixels_per_point: f32) -> Clip {
    let Primitive::Mesh(mesh) = &primitive.primitive else {
        return Clip::frame(frame);
    };

    Clip::around(
        mesh.calc_bounds().intersect(primitive.clip_rect),
        pixels_per_point,
        frame,
    )
}

/// Fills the pixels whose centers lie inside the triangle: top and left edges inclusive, bottom
/// and right edges exclusive, so triangles sharing an edge never blend a pixel twice.
fn draw_triangle(band: &mut Band<'_>, texture: &Texture, clip: Clip, mut points: [Point; 3]) {
    // Sorting by position means an edge shared by two triangles is always evaluated from the
    // same end, so both agree on exactly which pixels it covers.
    points.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    let [top, middle, bottom] = points;

    let area = (middle.x - top.x) * (bottom.y - top.y) - (bottom.x - top.x) * (middle.y - top.y);
    if area == 0.0 || !area.is_finite() {
        return;
    }

    // Every value changes linearly across the triangle: value = start + dx * x + dy * y.
    let mut dx = [0.0; 6];
    let mut dy = [0.0; 6];
    for i in 0..6 {
        let to_middle = middle.values[i] - top.values[i];
        let to_bottom = bottom.values[i] - top.values[i];
        dx[i] = (to_middle * (bottom.y - top.y) - to_bottom * (middle.y - top.y)) / area;
        dy[i] = (to_bottom * (middle.x - top.x) - to_middle * (bottom.x - top.x)) / area;
    }

    // Rectangles and lines have the same color and texture coordinates at every corner.
    #[expect(clippy::float_cmp, reason = "the values are copied from the vertices, not computed")]
    let solid = [middle, bottom]
        .iter()
        .all(|p| p.values == top.values)
        .then(|| shade(texture, top.values));

    // An untinted image whose rows map onto texture rows (like the overlay's screenshot) is
    // copied texel by texel instead of being shaded per pixel.
    let image = texture.filter == TextureFilter::Nearest
        && dx[5] == 0.0
        && [top, middle, bottom].iter().all(|p| p.values[..4] == [255.0; 4]);

    // Shadows, gradients and every anti-aliased edge take one color from the texture and vary
    // only the tint, so the texture is sampled once for the whole triangle.
    let texel = (dx[4] == 0.0 && dy[4] == 0.0 && dx[5] == 0.0 && dy[5] == 0.0)
        .then(|| texture.sample(top.values[4], top.values[5]));

    let first_row = pixel_index(top.y).max(clip.min_y);
    let end_row = pixel_index(bottom.y).min(clip.max_y);

    for y in first_row..end_row {
        let center_y = y as f32 + 0.5;
        let long = edge_x(top, bottom, center_y);
        let short = if center_y < middle.y {
            edge_x(top, middle, center_y)
        } else {
            edge_x(middle, bottom, center_y)
        };

        let first = pixel_index(long.min(short)).max(clip.min_x);
        let end = pixel_index(long.max(short)).min(clip.max_x);
        if first >= end {
            continue;
        }

        let row = (y as usize - band.top) * band.width;
        let span = &mut band.pixels[row + first as usize..row + end as usize];

        if let Some(color) = solid {
            blend_span(span, color);
            continue;
        }

        let offset_x = first as f32 + 0.5 - top.x;
        let offset_y = center_y - top.y;
        let mut values: [f32; 6] = std::array::from_fn(|i| top.values[i] + dx[i] * offset_x + dy[i] * offset_y);

        if image {
            copy_image_span(span, texture, values[4], dx[4], values[5]);
            continue;
        }

        for pixel in span {
            let color = match texel {
                Some(texel) => tint_texel(texel, &values),
                None => shade(texture, values),
            };
            blend(pixel, color);

            for (value, step) in values.iter_mut().zip(dx) {
                *value += step;
            }
        }
    }
}

/// Draws one texture row onto a span with nearest sampling; `u` is the texture coordinate at
/// the first pixel's center and `step` its change per pixel.
fn copy_image_span(span: &mut [u32], texture: &Texture, u: f32, step: f32, v: f32) {
    let row = ((v * texture.height as f32) as usize).min(texture.height - 1) * texture.width;
    let texels = &texture.pixels[row..row + texture.width];
    let width = texture.width as f32;

    let draw = |pixel: &mut u32, texel: Color32| blend(pixel, texel.to_array().map(u32::from));

    // One texel per pixel (the overlay's screenshot on its own monitor): a straight copy.
    if ((step * width) - 1.0).abs() < 1e-3 {
        let start = ((u * width) as usize).min(texels.len() - 1);
        let (copied, rest) = span.split_at_mut(span.len().min(texels.len() - start));

        for (pixel, &texel) in copied.iter_mut().zip(&texels[start..]) {
            draw(pixel, texel);
        }

        for pixel in rest {
            draw(pixel, texels[texels.len() - 1]);
        }
        return;
    }

    // Fixed-point texel position, computed from the start so rounding errors don't add up.
    let start = f64::from(u * width) * 65536.0;
    let step = f64::from(step * width) * 65536.0;
    let last = texels.len() - 1;

    for (i, pixel) in span.iter_mut().enumerate() {
        let x = (start + step * i as f64).max(0.0) as usize >> 16;
        draw(pixel, texels[x.min(last)]);
    }
}

/// First pixel whose center is at or past `coordinate`.
fn pixel_index(coordinate: f32) -> i32 {
    (coordinate - 0.5).ceil().clamp(i32::MIN as f32, i32::MAX as f32) as i32
}

/// Horizontal position of the edge from `start` to `end` (sorted) at height `y`.
fn edge_x(start: Point, end: Point, y: f32) -> f32 {
    let height = end.y - start.y;
    if height == 0.0 {
        return start.x;
    }

    start.x + (y - start.y) * (end.x - start.x) / height
}

/// Vertex color multiplied by the texture color, premultiplied RGBA.
fn shade(texture: &Texture, values: [f32; 6]) -> [u32; 4] {
    tint_texel(texture.sample(values[4], values[5]), &values)
}

/// Multiplies a texel by the color interpolated across the triangle.
fn tint_texel(texel: [u8; 4], values: &[f32; 6]) -> [u32; 4] {
    let tint = [values[0], values[1], values[2], values[3]].map(|c| c.clamp(0.0, 255.0).round() as u32);

    std::array::from_fn(|i| (u32::from(texel[i]) * tint[i] + 127) / 255)
}

impl Texture {
    fn sample(&self, u: f32, v: f32) -> [u8; 4] {
        let x = u * self.width as f32;
        let y = v * self.height as f32;

        match self.filter {
            TextureFilter::Nearest => self.texel(x.floor() as i64, y.floor() as i64),
            TextureFilter::Linear => self.bilinear(x - 0.5, y - 0.5),
        }
    }

    fn bilinear(&self, x: f32, y: f32) -> [u8; 4] {
        let (left, top) = (x.floor(), y.floor());
        let (fx, fy) = (x - left, y - top);
        let (left, top) = (left as i64, top as i64);

        let corners = [
            (self.texel(left, top), (1.0 - fx) * (1.0 - fy)),
            (self.texel(left + 1, top), fx * (1.0 - fy)),
            (self.texel(left, top + 1), (1.0 - fx) * fy),
            (self.texel(left + 1, top + 1), fx * fy),
        ];

        std::array::from_fn(|i| {
            let value: f32 = corners.iter().map(|(texel, weight)| f32::from(texel[i]) * weight).sum();
            value.round().clamp(0.0, 255.0) as u8
        })
    }

    /// Texel at the given position, clamped to the edges.
    fn texel(&self, x: i64, y: i64) -> [u8; 4] {
        let x = x.clamp(0, self.width as i64 - 1) as usize;
        let y = y.clamp(0, self.height as i64 - 1) as usize;

        self.pixels.get(y * self.width + x).map_or([0; 4], Color32::to_array)
    }
}

fn blend_span(span: &mut [u32], color: [u32; 4]) {
    if color[3] == 255 {
        span.fill(pack(color));
        return;
    }

    // The same color over every pixel: no per-pixel branches, so the loop vectorizes.
    let [r, g, b, a] = color;
    let keep = 255 - a;
    let over = |source: u32, pixel: u32, shift: u32| (source + (((pixel >> shift) & 0xFF) * keep + 127) / 255).min(255);

    for pixel in span {
        *pixel = (over(r, *pixel, 16) << 16) | (over(g, *pixel, 8) << 8) | over(b, *pixel, 0);
    }
}

/// Draws a premultiplied color over the pixel.
fn blend(pixel: &mut u32, color: [u32; 4]) {
    let [r, g, b, a] = color;
    if a == 255 {
        *pixel = pack(color);
        return;
    }
    if a == 0 && r == 0 && g == 0 && b == 0 {
        return;
    }

    let keep = 255 - a;
    let over = |source: u32, shift: u32| (source + (((*pixel >> shift) & 0xFF) * keep + 127) / 255).min(255);

    *pixel = (over(r, 16) << 16) | (over(g, 8) << 8) | over(b, 0);
}

fn pack([r, g, b, _]: [u32; 4]) -> u32 {
    (r.min(255) << 16) | (g.min(255) << 8) | b.min(255)
}

#[cfg(test)]
mod tests {
    use egui::TextureOptions;
    use egui::epaint::{ColorImage, ImageDelta};
    use egui::{Pos2, pos2, vec2};

    use super::*;

    const RED: Color32 = Color32::from_rgb(255, 0, 0);

    fn render(mesh: Mesh, width: usize, height: usize) -> Vec<u32> {
        let mut renderer = Renderer::default();
        let mut textures = TexturesDelta::default();
        let white = ColorImage::new([1, 1], vec![Color32::WHITE]);
        textures
            .set
            .entry(TextureId::default())
            .or_default()
            .push(ImageDelta::full(white, TextureOptions::NEAREST));

        let mut pixels = vec![0; width * height];
        let mut frame = Frame {
            pixels: &mut pixels,
            width,
            height,
        };
        let primitive = ClippedPrimitive {
            clip_rect: Rect::EVERYTHING,
            primitive: Primitive::Mesh(mesh),
        };
        renderer.render(&mut frame, &[primitive], &mut textures, 1.0);

        pixels
    }

    fn rect_mesh(min: Pos2, max: Pos2, color: Color32) -> Mesh {
        let mut mesh = Mesh::default();
        mesh.add_colored_rect(Rect::from_min_max(min, max), color);
        mesh
    }

    #[test]
    fn fills_exactly_the_covered_pixels() {
        let pixels = render(rect_mesh(pos2(1.0, 1.0), pos2(3.0, 2.0), RED), 4, 3);

        let red = 0x00FF_0000;
        assert_eq!(pixels, [0, 0, 0, 0, 0, red, red, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn blends_the_shared_diagonal_once() {
        let half_black = Color32::from_black_alpha(128);
        let mut mesh = rect_mesh(pos2(0.0, 0.0), pos2(4.0, 4.0), Color32::WHITE);
        mesh.append(rect_mesh(pos2(0.0, 0.0), pos2(4.0, 4.0), half_black));

        let pixels = render(mesh, 4, 4);

        assert!(pixels.iter().all(|&p| p == pixels[0]), "uneven fill: {pixels:x?}");
        assert_eq!(pixels[0], 0x007F_7F7F);
    }

    /// A background covering the frame, a translucent band over it and a small card, the way the
    /// overlay draws a screenshot, its dimming and the result card.
    fn scene(card: Pos2) -> Vec<ClippedPrimitive> {
        let meshes = [
            rect_mesh(pos2(0.0, 0.0), pos2(64.0, 64.0), Color32::from_rgb(20, 60, 120)),
            rect_mesh(pos2(0.0, 0.0), pos2(64.0, 20.0), Color32::from_black_alpha(150)),
            rect_mesh(card, card + vec2(18.0, 12.0), Color32::from_rgb(30, 30, 33)),
            rect_mesh(card + vec2(2.0, 2.0), card + vec2(9.5, 7.5), RED.gamma_multiply(0.5)),
        ];

        meshes
            .into_iter()
            .map(|mesh| ClippedPrimitive {
                clip_rect: Rect::EVERYTHING,
                primitive: Primitive::Mesh(mesh),
            })
            .collect()
    }

    fn draw_scenes(scenes: &[Vec<ClippedPrimitive>], width: usize, height: usize) -> Vec<u32> {
        let mut renderer = Renderer::default();
        let mut pixels = vec![0; width * height];

        for (index, primitives) in scenes.iter().enumerate() {
            let mut textures = TexturesDelta::default();
            if index == 0 {
                let white = ColorImage::new([1, 1], vec![Color32::WHITE]);
                textures
                    .set
                    .entry(TextureId::default())
                    .or_default()
                    .push(ImageDelta::full(white, TextureOptions::NEAREST));
            }

            let mut frame = Frame {
                pixels: &mut pixels,
                width,
                height,
            };
            renderer.render(&mut frame, primitives, &mut textures, 1.0);
        }

        pixels
    }

    #[test]
    fn redrawing_only_what_changed_matches_drawing_everything() {
        let (width, height) = (64, 64);
        let moves = [pos2(10.0, 30.0), pos2(11.5, 31.0), pos2(30.0, 44.0), pos2(30.0, 44.0)];
        let scenes: Vec<_> = moves.iter().map(|card| scene(*card)).collect();

        let step_by_step = draw_scenes(&scenes, width, height);
        let from_scratch = draw_scenes(&scenes[scenes.len() - 1..], width, height);

        let differences = step_by_step
            .iter()
            .zip(&from_scratch)
            .filter(|(drawn, expected)| drawn != expected)
            .count();
        assert_eq!(differences, 0);
    }

    #[test]
    fn a_frame_that_did_not_change_is_left_alone() {
        let (width, height) = (64, 64);
        let scenes = [scene(pos2(10.0, 30.0)), scene(pos2(10.0, 30.0))];
        let mut renderer = Renderer::default();
        let mut pixels = vec![0; width * height];
        let mut textures = TexturesDelta::default();

        let white = ColorImage::new([1, 1], vec![Color32::WHITE]);
        textures
            .set
            .entry(TextureId::default())
            .or_default()
            .push(ImageDelta::full(white, TextureOptions::NEAREST));

        let mut render = |primitives: &[ClippedPrimitive], textures: &mut TexturesDelta| {
            let mut frame = Frame {
                pixels: &mut pixels,
                width,
                height,
            };

            renderer.render(&mut frame, primitives, textures, 1.0)
        };

        assert!(render(&scenes[0], &mut textures).is_some());
        assert_eq!(render(&scenes[1], &mut TexturesDelta::default()), None);
    }

    #[test]
    fn moving_a_card_only_damages_the_card() {
        let (width, height) = (64, 64);
        let mut renderer = Renderer::default();
        let mut pixels = vec![0; width * height];
        let mut textures = TexturesDelta::default();

        let white = ColorImage::new([1, 1], vec![Color32::WHITE]);
        textures
            .set
            .entry(TextureId::default())
            .or_default()
            .push(ImageDelta::full(white, TextureOptions::NEAREST));

        let mut render = |primitives: &[ClippedPrimitive], textures: &mut TexturesDelta| {
            let mut frame = Frame {
                pixels: &mut pixels,
                width,
                height,
            };

            renderer.render(&mut frame, primitives, textures, 1.0)
        };

        render(&scene(pos2(10.0, 30.0)), &mut textures);
        let damage = render(&scene(pos2(12.0, 30.0)), &mut TexturesDelta::default());

        // The two card positions, with a pixel of slack around them.
        let expected = Damage {
            rows: 29..43,
            columns: 9..31,
        };
        assert_eq!(damage, Some(expected));
    }
}
