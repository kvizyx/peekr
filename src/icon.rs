//! The app icon, rendered from `assets/icon.svg` by `cargo xtask icon`.

const PNG: &[u8] = include_bytes!("../assets/icon.png");

pub const SIZE: u32 = 32;

/// The icon as unpremultiplied RGBA pixels, `SIZE` × `SIZE`.
pub fn rgba() -> Vec<u8> {
    let icon = image::load_from_memory_with_format(PNG, image::ImageFormat::Png)
        .expect("the embedded icon is a valid PNG")
        .into_rgba8();

    debug_assert_eq!(icon.dimensions(), (SIZE, SIZE), "assets/icon.png has the wrong size");

    icon.into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_matches_the_declared_size() {
        assert_eq!(rgba().len(), (SIZE * SIZE * 4) as usize);
    }
}
