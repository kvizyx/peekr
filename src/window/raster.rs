//! CPU rasterizer for the triangle meshes egui produces.
//!
//! egui tessellates everything into textured, vertex-colored triangles with premultiplied alpha
//! and anti-aliases edges itself (feathering), so a plain scanline rasterizer that blends in
//! gamma space draws it the same way the GPU backends do.

use std::collections::HashMap;
use std::num::NonZeroUsize;

use egui::epaint::textures::{TextureFilter, TexturesDelta};
use egui::epaint::{ClippedPrimitive, ImageData, ImageDelta, Mesh, Primitive, Vertex};
use egui::{Color32, Rect, TextureId};

/// Frames smaller than this are drawn on the calling thread; spawning threads would cost more.
const PARALLEL_MIN_PIXELS: usize = 256 * 256;
const MAX_THREADS: usize = 8;

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
}

struct Texture {
    width: usize,
    height: usize,
    pixels: Vec<Color32>,
    filter: TextureFilter,
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
    /// Clears the frame and draws the primitives, applying (and consuming) texture changes.
    pub fn render(
        &mut self,
        frame: &mut Frame<'_>,
        primitives: &[ClippedPrimitive],
        textures: &mut TexturesDelta,
        pixels_per_point: f32,
    ) {
        for (id, deltas) in textures.set.drain() {
            for delta in deltas {
                self.set_texture(id, &delta);
            }
        }

        if frame.width > 0 {
            self.draw(frame, primitives, pixels_per_point);
        }

        for id in textures.free.drain() {
            self.textures.remove(&id);
        }
    }

    /// Splits large frames into horizontal bands drawn on separate threads.
    fn draw(&self, frame: &mut Frame<'_>, primitives: &[ClippedPrimitive], pixels_per_point: f32) {
        let threads = if frame.pixels.len() < PARALLEL_MIN_PIXELS {
            1
        } else {
            std::thread::available_parallelism()
                .map_or(1, NonZeroUsize::get)
                .min(MAX_THREADS)
        };
        let band_rows = frame.height.div_ceil(threads).max(1);

        std::thread::scope(|scope| {
            for (index, pixels) in frame.pixels.chunks_mut(band_rows * frame.width).enumerate() {
                let mut band = Band {
                    pixels,
                    width: frame.width,
                    top: index * band_rows,
                };

                if threads == 1 {
                    self.draw_band(&mut band, primitives, pixels_per_point);
                } else {
                    scope.spawn(move || self.draw_band(&mut band, primitives, pixels_per_point));
                }
            }
        });
    }

    fn draw_band(&self, band: &mut Band<'_>, primitives: &[ClippedPrimitive], pixels_per_point: f32) {
        band.pixels.fill(0);

        for primitive in primitives {
            let Primitive::Mesh(mesh) = &primitive.primitive else {
                continue;
            };

            let clip = Clip::new(primitive.clip_rect, pixels_per_point, band);
            if clip.min_x < clip.max_x && clip.min_y < clip.max_y {
                self.draw_mesh(band, mesh, clip, pixels_per_point);
            }
        }
    }

    fn set_texture(&mut self, id: TextureId, delta: &ImageDelta) {
        let ImageData::Color(image) = &delta.image;
        let [width, height] = image.size;

        let Some([x, y]) = delta.pos else {
            let texture = Texture {
                width,
                height,
                pixels: image.pixels.clone(),
                filter: delta.options.magnification,
            };
            self.textures.insert(id, texture);
            return;
        };

        let Some(texture) = self.textures.get_mut(&id) else {
            return;
        };

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
            blend(pixel, shade(texture, values));

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
fn shade(texture: &Texture, [red, green, blue, alpha, u, v]: [f32; 6]) -> [u32; 4] {
    let texel = texture.sample(u, v);
    let tint = [red, green, blue, alpha].map(|c| c.clamp(0.0, 255.0).round() as u32);

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
    use egui::{Pos2, pos2};

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
}
