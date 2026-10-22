//! Rasterise `assets/icon/*.svg` into the PNGs the capture overlay draws (docs/23 §14.54).
//!
//! **Why bake instead of parsing SVG at run time.** The panel's glyphs have to look exactly like the
//! design's SVG files, and two attempts to draw the SVG itself inside the overlay failed: Direct2D's
//! `DrawSvgDocument` paints nothing at all on this machine (measured, including a hand-written
//! minimal SVG — the whole story is in `assets/icon/README.md`), and redrawing the shapes by hand
//! with lines is, by definition, not the same picture. So a real SVG renderer (resvg) is asked to
//! draw the files, once, here; the overlay only has to blit the result.
//!
//! The output is **white on transparent**: the overlay tints it at draw time with a Direct2D colour
//! matrix, so one PNG serves every state.
//!
//! Run `cargo run -p icon-bake` after changing any `.svg`. It is deterministic — the same input
//! produces byte-identical output — and the tests below fail if the committed PNGs drift from the
//! SVGs.

use std::path::{Path, PathBuf};

/// The baked bitmap's edge, in pixels.
///
/// The icons are 24 units; 96 px is 4×, which covers up to 400 % display scaling and means every
/// smaller draw is a supersampled downscale rather than a naive one.
pub const ICON_BAKED_PX: u32 = 96;

/// The icons' own coordinate system (`viewBox="0 0 24 24"`).
pub const ICON_UNITS: f32 = 24.0;

/// The colour `currentColor` is resolved to, in the SVG and therefore in the PNG.
///
/// Every Tabler icon paints with `currentColor` (a CSS value no non-browser renderer resolves), and
/// the overlay's tinting wants a neutral white to multiply.
pub const BAKED_COLOUR: &str = "#ffffff";

fn main() -> Result<(), String> {
    let written = bake_all()?;
    for path in &written {
        println!("baked {}", path.display());
    }
    println!("{} icons up to date", written.len());
    Ok(())
}

/// `assets/icon`, found from this crate's manifest so the current directory does not matter.
pub fn icon_dir() -> Result<PathBuf, String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icon");
    dir.canonicalize()
        .map_err(|error| format!("{}: {error}", dir.display()))
}

/// The `.svg` files in `assets/icon`, in name order (so the log and any diff are stable).
pub fn svg_paths() -> Result<Vec<PathBuf>, String> {
    let dir = icon_dir()?;
    let mut svgs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|error| format!("{}: {error}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "svg"))
        .collect();
    svgs.sort();
    if svgs.is_empty() {
        return Err(format!("no .svg files in {}", dir.display()));
    }
    Ok(svgs)
}

/// Rewrite every PNG and every `.rgba` next to its SVG, and report the paths written.
pub fn bake_all() -> Result<Vec<PathBuf>, String> {
    let mut written = Vec::new();
    for svg in svg_paths()? {
        let png = svg.with_extension("png");
        std::fs::write(&png, render(&svg)?).map_err(|error| format!("{}: {error}", png.display()))?;
        written.push(png);
        let rgba = svg.with_extension("rgba");
        std::fs::write(&rgba, render_rgba(&svg)?)
            .map_err(|error| format!("{}: {error}", rgba.display()))?;
        written.push(rgba);
    }
    Ok(written)
}

/// One SVG → the raw pixels, for renderers that cannot use a colour matrix effect.
///
/// The save panel paints through a DC render target, which has no device context and therefore no
/// effect graph, so it tints the pixels itself. The pixels are written **straight**, not
/// premultiplied, which is what the tinting function on that side expects; `tiny_skia` hands them
/// over premultiplied, so each channel is divided back out by its alpha here.
pub fn render_rgba(svg: &Path) -> Result<Vec<u8>, String> {
    let mut pixmap = render_pixmap(svg)?;
    for pixel in pixmap.data_mut().chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 {
            pixel.copy_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * 255) / alpha).min(255) as u8;
        }
    }
    Ok(pixmap.take())
}

/// One SVG → the PNG bytes the overlay loads, in memory.
pub fn render(svg: &Path) -> Result<Vec<u8>, String> {
    render_pixmap(svg)?
        .encode_png()
        .map_err(|error| format!("{}: {error}", svg.display()))
}

/// One SVG → the rendered pixmap, before it is encoded either way.
fn render_pixmap(svg: &Path) -> Result<resvg::tiny_skia::Pixmap, String> {
    let source =
        std::fs::read_to_string(svg).map_err(|error| format!("{}: {error}", svg.display()))?;
    // `currentColor` is not SVG, it is CSS: resolve it before the file reaches any renderer.
    let source = source.replace("currentColor", BAKED_COLOUR);
    let tree = resvg::usvg::Tree::from_str(&source, &resvg::usvg::Options::default())
        .map_err(|error| format!("{}: {error}", svg.display()))?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(ICON_BAKED_PX, ICON_BAKED_PX)
        .ok_or_else(|| format!("{}: cannot allocate a {ICON_BAKED_PX}px pixmap", svg.display()))?;
    // A fresh pixmap is fully transparent, which is the background the icons want.
    let scale = ICON_BAKED_PX as f32 / ICON_UNITS;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one that matters: the PNGs in the repository are exactly what the SVGs render to.
    ///
    /// A designer can change an `.svg` and forget to re-bake; without this test the overlay would
    /// keep drawing yesterday's picture and nothing would say so.
    #[test]
    fn the_committed_pngs_are_what_the_svgs_render_to() {
        for svg in svg_paths().unwrap() {
            let png = svg.with_extension("png");
            let committed = std::fs::read(&png).unwrap_or_else(|error| {
                panic!(
                    "{} is missing ({error}); run `cargo run -p icon-bake`",
                    png.display()
                )
            });
            assert_eq!(
                committed,
                render(&svg).unwrap(),
                "{} is stale; run `cargo run -p icon-bake`",
                png.display()
            );
        }
    }

    /// Baking is deterministic: the same SVG renders to the same bytes twice in a row.
    #[test]
    fn baking_the_same_icon_twice_gives_the_same_bytes() {
        let svg = svg_paths().unwrap().remove(0);
        assert_eq!(render(&svg).unwrap(), render(&svg).unwrap());
    }

    /// The PNGs must be 96×96 RGBA — the overlay's bitmap and colour matrix assume it.
    #[test]
    fn the_baked_icons_are_96px_rgba() {
        let png = render(&svg_paths().unwrap().remove(0)).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
        assert_eq!(&png[12..16], b"IHDR");
        let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!((width, height), (ICON_BAKED_PX, ICON_BAKED_PX));
        assert_eq!(png[25], 6, "colour type must be RGBA");
    }

    /// The committed `.rgba` files are the same render, written straight rather than premultiplied.
    ///
    /// The panel divides the alpha back out of `tiny_skia`'s premultiplied pixels before writing, so
    /// a fully opaque white pixel stays 255 and a transparent one stays zero.
    #[test]
    fn the_committed_raw_pixels_are_straight_and_current() {
        for svg in svg_paths().unwrap() {
            let rgba = svg.with_extension("rgba");
            let committed = std::fs::read(&rgba).unwrap_or_else(|error| {
                panic!(
                    "{} is missing ({error}); run `cargo run -p icon-bake`",
                    rgba.display()
                )
            });
            assert_eq!(
                committed.len(),
                (ICON_BAKED_PX * ICON_BAKED_PX * 4) as usize,
                "{}",
                rgba.display()
            );
            assert_eq!(
                committed,
                render_rgba(&svg).unwrap(),
                "{} is stale; run `cargo run -p icon-bake`",
                rgba.display()
            );
        }
    }

    /// A pixel with ink keeps its alpha and stays white; the corners stay transparent.
    #[test]
    fn raw_pixels_are_white_where_there_is_ink() {
        let pixels = render_rgba(&svg_paths().unwrap().remove(0)).unwrap();
        let opaque = pixels
            .chunks_exact(4)
            .find(|pixel| pixel[3] == 255)
            .expect("an icon has some fully opaque ink");
        assert_eq!(&opaque[..3], &[255, 255, 255], "ink is white before tinting");
        assert!(
            pixels.chunks_exact(4).any(|pixel| pixel[3] == 0),
            "an icon has transparent surroundings"
        );
    }
}
