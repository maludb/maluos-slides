//! `cargo xtask icons <source.png>`: derive every app icon from one square PNG (the owner's
//! MaluOS Slides mark): the hicolor sizes under `assets/app-icon/hicolor/<n>x<n>/apps/`, the 1024 px
//! copy, the scalable entry (an SVG that embeds the 512 px PNG) and the Windows/web `.ico` from
//! the sizes up to 256 px. Lanczos resampling; the source must be square with transparent corners.

use std::path::Path;

const HICOLOR: &[u32] = &[16, 24, 32, 48, 64, 128, 256, 512];
const ICO: &[u32] = &[16, 24, 32, 48, 64, 128, 256];
const APP_ID: &str = "org.maluos.slides";

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let [source] = args else { return Err("usage: cargo xtask icons <source.png>".into()) };
    let bytes = std::fs::read(source).map_err(|e| format!("{source}: {e}"))?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("{source}: {e}"))?.to_rgba8();
    let (w, h) = img.dimensions();
    if w != h {
        return Err(format!("{source}: the icon must be square, got {w}x{h}"));
    }
    let corners = [img.get_pixel(0, 0), img.get_pixel(w - 1, 0), img.get_pixel(0, h - 1), img.get_pixel(w - 1, h - 1)];
    if corners.iter().any(|p| p.0[3] != 0) {
        return Err(format!("{source}: the corners are not transparent ({:?})", corners.map(|p| p.0)));
    }
    let dir = root.join("assets/app-icon");
    let mut pngs = Vec::new();
    for &n in HICOLOR.iter().chain(std::iter::once(&1024)) {
        let scaled = image::imageops::resize(&img, n, n, image::imageops::FilterType::Lanczos3);
        let mut out = std::io::Cursor::new(Vec::new());
        scaled.write_to(&mut out, image::ImageFormat::Png).map_err(|e| format!("encoding {n} px: {e}"))?;
        let png = out.into_inner();
        let path = if n == 1024 { dir.join("maluos-slides-1024.png") } else { dir.join(format!("hicolor/{n}x{n}/apps/{APP_ID}.png")) };
        write(&path, &png)?;
        if ICO.contains(&n) {
            pngs.push(png.clone());
        }
        if n == 512 {
            let svg = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"512\" height=\"512\" viewBox=\"0 0 512 512\">\n  <image width=\"512\" height=\"512\" xlink:href=\"data:image/png;base64,{}\"/>\n</svg>\n",
                base64(&png)
            );
            write(&dir.join(format!("hicolor/scalable/apps/{APP_ID}.svg")), svg.as_bytes())?;
        }
    }
    write(&dir.join("maluos-slides.ico"), &crate::ico::pack(&pngs)?)?;
    Ok(())
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("wrote {} ({} bytes)", path.display(), bytes.len());
    Ok(())
}

/// Standard base64 (RFC 4648) with padding; small enough not to need a dependency.
fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                s.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_the_reference_vectors() {
        for (input, want) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(super::base64(input.as_bytes()), want);
        }
    }
}
