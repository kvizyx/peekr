//! The app icon, drawn in code to avoid shipping image assets.

pub const SIZE: u32 = 32;

/// A rounded blue square with three white "text lines", as unpremultiplied RGBA pixels.
pub fn rgba() -> Vec<u8> {
    const BACKGROUND: [u8; 3] = [40, 120, 230];
    const FOREGROUND: [u8; 3] = [255, 255, 255];
    /// Text lines as (first row, end column); each is 3 px tall and starts at column 8.
    const LINES: [(u32, u32); 3] = [(9, 23), (15, 23), (21, 17)];

    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];

    for y in 0..SIZE {
        for x in 0..SIZE {
            // Signed distance from the pixel center to a rounded square with corner radius 7.
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let (qx, qy) = ((fx - 16.0).abs() - 9.0, (fy - 16.0).abs() - 9.0);
            let distance = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - 7.0;

            let alpha = (0.5 - distance).clamp(0.0, 1.0);
            if alpha == 0.0 {
                continue;
            }

            let on_line = LINES
                .iter()
                .any(|&(row, end)| (row..row + 3).contains(&y) && (8..end).contains(&x));
            let [r, g, b] = if on_line { FOREGROUND } else { BACKGROUND };

            let i = ((y * SIZE + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[r, g, b, (alpha * 255.0) as u8]);
        }
    }

    rgba
}
