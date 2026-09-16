//! Image preparation shared by the detection and recognition models.

use image::{Rgb, RgbImage, RgbaImage};

/// Converts an RGB image into a planar BGR float tensor (PaddleOCR models are trained on BGR).
///
/// `normalize` receives the output channel index (0 = blue, 2 = red) and the raw 0..=255 value.
pub fn to_bgr_chw(img: &RgbImage, normalize: impl Fn(usize, f32) -> f32) -> Vec<f32> {
    let (w, h) = img.dimensions();
    let plane = (w * h) as usize;
    let mut data = vec![0f32; plane * 3];

    for (i, Rgb([r, g, b])) in img.pixels().enumerate() {
        data[i] = normalize(0, f32::from(*b));
        data[plane + i] = normalize(1, f32::from(*g));
        data[2 * plane + i] = normalize(2, f32::from(*r));
    }

    data
}

/// Drops alpha and adds a `pad`-pixel border filled with the average color of the image edge,
/// so text touching the edges is still detected.
pub fn pad_with_border_color(img: &RgbaImage, pad: u32) -> RgbImage {
    let (width, height) = img.dimensions();

    let mut sum = [0u64; 3];
    let mut count = 0u64;
    for (x, y, px) in img.enumerate_pixels() {
        if x == 0 || y == 0 || x == width - 1 || y == height - 1 {
            for (acc, &value) in sum.iter_mut().zip(&px.0) {
                *acc += u64::from(value);
            }
            count += 1;
        }
    }
    let fill = Rgb(sum.map(|s| (s / count.max(1)) as u8));

    let mut out = RgbImage::from_pixel(width + 2 * pad, height + 2 * pad, fill);
    for (x, y, px) in img.enumerate_pixels() {
        let [red, green, blue, _] = px.0;
        out.put_pixel(x + pad, y + pad, Rgb([red, green, blue]));
    }

    out
}

#[cfg(test)]
mod tests {
    use image::Rgba;

    use super::*;

    #[test]
    fn channels_are_reordered_to_bgr_planes() {
        let img = RgbImage::from_raw(2, 1, vec![10, 20, 30, 40, 50, 60]).expect("buffer matches size");

        let data = to_bgr_chw(&img, |_, v| v);

        assert_eq!(data, vec![30.0, 60.0, 20.0, 50.0, 10.0, 40.0]);
    }

    #[test]
    fn padding_uses_the_edge_color() {
        let img = RgbaImage::from_pixel(3, 2, Rgba([100, 150, 200, 255]));

        let padded = pad_with_border_color(&img, 2);

        assert_eq!(padded.dimensions(), (7, 6));
        assert_eq!(padded.get_pixel(0, 0), &Rgb([100, 150, 200]));
        assert_eq!(padded.get_pixel(3, 3), &Rgb([100, 150, 200]));
    }
}
