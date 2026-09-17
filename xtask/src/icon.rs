//! Renders the app icon from its SVG source, so the app itself ships a ready PNG.

use std::path::Path;

use anyhow::{Context as _, Result};
use resvg::{tiny_skia, usvg};

/// Tray and window icons are drawn at this size; the system scales them as needed.
const SIZE: u32 = 32;

pub fn render(root: &Path) -> Result<()> {
    let source = root.join("assets/icon.svg");
    let output = root.join(format!("assets/icon-{SIZE}.png"));

    let data = std::fs::read(&source).with_context(|| format!("reading {}", source.display()))?;
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).context("parsing the icon SVG")?;

    let mut pixmap = tiny_skia::Pixmap::new(SIZE, SIZE).context("allocating the icon pixmap")?;
    let scale = SIZE as f32 / tree.size().width().max(tree.size().height());
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    pixmap
        .save_png(&output)
        .with_context(|| format!("writing {}", output.display()))?;
    eprintln!("rendered {} ({SIZE}x{SIZE})", output.display());

    Ok(())
}
