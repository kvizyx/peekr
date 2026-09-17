//! Renders the app icon from its SVG source: a PNG the app embeds and an ICO for the Windows
//! installer and its shortcuts.

use std::path::Path;

use anyhow::{Context as _, Result};
use image::codecs::ico::{IcoEncoder, IcoFrame};
use resvg::{tiny_skia, usvg};

/// Tray and window icons are drawn at this size; the system scales them as needed.
const PNG_SIZE: u32 = 32;
/// Windows picks the closest of these, from list views to the desktop.
const ICO_SIZES: [u32; 5] = [16, 32, 48, 128, 256];

pub fn render(root: &Path) -> Result<()> {
    let source = root.join("assets/icon.svg");
    let data = std::fs::read(&source).with_context(|| format!("reading {}", source.display()))?;
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).context("parsing the icon SVG")?;

    let png = root.join(format!("assets/icon-{PNG_SIZE}.png"));
    draw(&tree, PNG_SIZE)?
        .save_png(&png)
        .with_context(|| format!("writing {}", png.display()))?;
    eprintln!("rendered {} ({PNG_SIZE}x{PNG_SIZE})", png.display());

    let ico = root.join("assets/icon.ico");
    write_ico(&tree, &ico)?;
    eprintln!("rendered {} ({ICO_SIZES:?})", ico.display());

    Ok(())
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
