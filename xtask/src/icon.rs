//! Renders the app icon from its SVG source: a PNG the app embeds, an ICO for the Windows
//! installer and its shortcuts, and, when a release is built, a PNG for the Linux `AppImage` and
//! an ICNS for the macOS app bundle.

use std::path::Path;

use anyhow::{Context as _, Result};
use image::codecs::ico::{IcoEncoder, IcoFrame};
use resvg::{tiny_skia, usvg};

/// Tray and window icons are drawn at this size; the system scales them as needed.
const PNG_SIZE: u32 = 32;
/// Windows picks the closest of these, from list views to the desktop.
const ICO_SIZES: [u32; 5] = [16, 32, 48, 128, 256];
/// What an ICNS holds: the type of each image and its size. Finder and the Dock pick the closest,
/// with the `@2x` types for Retina displays.
const ICNS_IMAGES: [(&[u8; 4], u32); 8] = [
    (b"ic11", 32),   // 16x16@2x
    (b"ic12", 64),   // 32x32@2x
    (b"ic07", 128),  // 128x128
    (b"ic13", 256),  // 128x128@2x
    (b"ic08", 256),  // 256x256
    (b"ic14", 512),  // 256x256@2x
    (b"ic09", 512),  // 512x512
    (b"ic10", 1024), // 512x512@2x
];

pub fn render(root: &Path) -> Result<()> {
    let tree = load(root)?;

    let png = root.join(format!("assets/icon-{PNG_SIZE}.png"));
    save_png(&tree, PNG_SIZE, &png)?;
    eprintln!("rendered {} ({PNG_SIZE}x{PNG_SIZE})", png.display());

    let ico = root.join("assets/icon.ico");
    write_ico(&tree, &ico)?;
    eprintln!("rendered {} ({ICO_SIZES:?})", ico.display());

    Ok(())
}

/// Renders the icon into a PNG of `size` pixels square.
pub fn png(root: &Path, size: u32, output: &Path) -> Result<()> {
    save_png(&load(root)?, size, output)
}

/// Renders the icon into an ICNS, the format a macOS app bundle takes its icon in.
pub fn icns(root: &Path, output: &Path) -> Result<()> {
    let tree = load(root)?;
    let mut images = Vec::with_capacity(ICNS_IMAGES.len());

    for (kind, size) in ICNS_IMAGES {
        let png = draw(&tree, size)?.encode_png().context("encoding the icon as PNG")?;
        images.push((*kind, png));
    }

    std::fs::write(output, encode_icns(&images)?).with_context(|| format!("writing {}", output.display()))
}

/// An ICNS file: a header, then one block per image, each its type, its length and PNG data,
/// with every length in big-endian and counting its own header.
fn encode_icns(images: &[([u8; 4], Vec<u8>)]) -> Result<Vec<u8>> {
    const HEADER: usize = 8;

    let length = |data: usize| u32::try_from(HEADER + data).context("the icon is too large for an ICNS");
    let total = images.iter().map(|(_, png)| HEADER + png.len()).sum();

    let mut icns = Vec::with_capacity(HEADER + total);
    icns.extend_from_slice(b"icns");
    icns.extend_from_slice(&length(total)?.to_be_bytes());

    for (kind, png) in images {
        icns.extend_from_slice(kind);
        icns.extend_from_slice(&length(png.len())?.to_be_bytes());
        icns.extend_from_slice(png);
    }

    Ok(icns)
}

fn load(root: &Path) -> Result<usvg::Tree> {
    let source = root.join("assets/icon.svg");
    let data = std::fs::read(&source).with_context(|| format!("reading {}", source.display()))?;

    usvg::Tree::from_data(&data, &usvg::Options::default()).context("parsing the icon SVG")
}

fn save_png(tree: &usvg::Tree, size: u32, output: &Path) -> Result<()> {
    draw(tree, size)?
        .save_png(output)
        .with_context(|| format!("writing {}", output.display()))
}

fn draw(tree: &usvg::Tree, size: u32) -> Result<tiny_skia::Pixmap> {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).context("allocating the icon pixmap")?;
    let scale = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    Ok(pixmap)
}

/// Writes every size into one ICO file, each as a PNG, which Windows has read since Vista.
fn write_ico(tree: &usvg::Tree, output: &Path) -> Result<()> {
    let mut frames = Vec::with_capacity(ICO_SIZES.len());

    for size in ICO_SIZES {
        let pixmap = draw(tree, size)?;
        let png = pixmap.encode_png().context("encoding the icon as PNG")?;
        frames.push(IcoFrame::with_encoded(
            png,
            size,
            size,
            image::ExtendedColorType::Rgba8,
        )?);
    }

    let file = std::fs::File::create(output).with_context(|| format!("writing {}", output.display()))?;
    IcoEncoder::new(file).encode_images(&frames)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icns_blocks_carry_their_type_and_length() {
        let icns = encode_icns(&[(*b"ic07", vec![1, 2, 3]), (*b"ic08", vec![4])]).expect("small icon");

        assert_eq!(
            icns,
            [
                b"icns".as_slice(),
                &28u32.to_be_bytes(),
                b"ic07",
                &11u32.to_be_bytes(),
                &[1, 2, 3],
                b"ic08",
                &9u32.to_be_bytes(),
                &[4],
            ]
            .concat()
        );
    }
}
