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
//! Two properties are enforced rather than assumed, both because a measurement said so:
//!
//! * **The canvas comes from the file.** The renderer is asked for the SVG's own size and scaled to
//!   fill the baked bitmap, so a 24-unit file and a 16-unit file both arrive full size. Hard-coding
//!   24 units used to mean a 16-unit file baked at two thirds scale in the top-left corner, and the
//!   workaround was wrapping the artwork in `scale(1.5)` by hand.
//! * **The artwork stays where the SVG puts it.** The canvas is scaled to fill, and nothing is
//!   moved afterwards: optical centring is not geometric centring, and the bake is not in a
//!   position to second-guess where the designer placed the drawing. An earlier version measured
//!   the ink and shifted it to the canvas centre, which silently moved three icons that look
//!   symmetric and are not; the rule is now that the PNG is the SVG, scaled.
//!
//! Run `cargo run -p icon-bake` after changing any `.svg`. It is deterministic — the same input
//! produces byte-identical output — and the tests below fail if the committed PNGs drift from the
//! SVGs.
//!
//! The PNG encoder is `tiny_skia`'s. A hand-written fixed-Huffman DEFLATE was measured against it
//! and produces files 35–60 % smaller, which for eighteen development-time assets is about fourteen
//! kilobytes: not worth a hundred and eighty lines of compression code that nothing else uses.

use std::path::{Path, PathBuf};

/// The baked bitmap's edge, in pixels.
///
/// The icons are 24 units; 96 px is 4×, which covers up to 400 % display scaling and means every
/// smaller draw is a supersampled downscale rather than a naive one.
pub const ICON_BAKED_PX: u32 = 96;

/// The colour `currentColor` is resolved to, in the SVG and therefore in the PNG.
///
/// Every Tabler icon paints with `currentColor` (a CSS value no non-browser renderer resolves), and
/// the overlay's tinting wants a neutral white to multiply.
pub const BAKED_COLOUR: &str = "#ffffff";

/// Elements whose rendering is not predictable enough to bake into a shipped asset.
///
/// The renderer would happily draw most of these, but two machines with different fonts, or a
/// different resvg minor version, would draw them differently — and the drift would be invisible,
/// because the committed-asset test compares this machine against itself.
const BLOCKED_TAGS: &[(&str, &str)] = &[
    ("text", "<text> is laid out with fonts that vary by machine"),
    ("tspan", "<tspan> is laid out with fonts that vary by machine"),
    ("textPath", "<textPath> is laid out with fonts that vary by machine"),
    ("mask", "<mask> is implemented differently across renderers"),
    ("foreignObject", "<foreignObject> is arbitrary markup"),
    ("image", "<image> would load a file that is not in the repository"),
    ("animate", "<animate> makes the frozen frame depend on timing"),
    ("animateTransform", "<animateTransform> makes the frozen frame depend on timing"),
    ("animateMotion", "<animateMotion> makes the frozen frame depend on timing"),
    ("set", "<set> makes the frozen frame depend on timing"),
];

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
    preflight(&source).map_err(|error| format!("{}: {error}", svg.display()))?;
    let tree = resvg::usvg::Tree::from_str(&source, &resvg::usvg::Options::default())
        .map_err(|error| format!("{}: {error}", svg.display()))?;

    // The canvas is the file's own size, not an assumed one: a 16-unit file has to arrive as large
    // as a 24-unit one. A non-square file is scaled by its longer edge and centred on the shorter.
    let size = tree.size();
    let unit = size.width().max(size.height());
    if !(unit > 0.0) {
        return Err(format!(
            "{}: the SVG has no size ({}×{})",
            svg.display(),
            size.width(),
            size.height()
        ));
    }
    let scale = ICON_BAKED_PX as f32 / unit;
    let dx = (ICON_BAKED_PX as f32 - size.width() * scale) / 2.0;
    let dy = (ICON_BAKED_PX as f32 - size.height() * scale) / 2.0;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(ICON_BAKED_PX, ICON_BAKED_PX)
        .ok_or_else(|| format!("{}: cannot allocate a {ICON_BAKED_PX}px pixmap", svg.display()))?;
    // A fresh pixmap is fully transparent, which is the background the icons want.
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

/// Reject SVG whose rendering is not predictable across machines.
///
/// Errors only, deliberately: a warning nobody reads is worse than a check that stops the bake.
fn preflight(source: &str) -> Result<(), String> {
    let document = roxmltree::Document::parse(source)
        .map_err(|error| format!("not well-formed XML: {error}"))?;
    let root = document.root_element();
    if root.tag_name().name() != "svg" {
        return Err(format!(
            "the root element is <{}>, not <svg>",
            root.tag_name().name()
        ));
    }
    let mut complaints = Vec::new();
    for node in document.descendants().filter(|node| node.is_element()) {
        let name = node.tag_name().name();
        for (tag, why) in BLOCKED_TAGS {
            if name == *tag {
                complaints.push((*why).to_string());
            }
        }
        if node.attribute("filter").is_some() {
            complaints.push("filter is implemented differently across renderers".to_string());
        }
        if let Some(style) = node.attribute("style") {
            if style.contains("filter") {
                complaints.push("an inline filter is implemented differently".to_string());
            }
        }
        for href in [
            node.attribute("href"),
            node.attribute(("http://www.w3.org/1999/xlink", "href")),
        ]
        .into_iter()
        .flatten()
        {
            if !href.is_empty() && !href.starts_with('#') {
                complaints.push(format!("a reference to {href} is not in this repository"));
            }
        }
    }
    complaints.sort();
    complaints.dedup();
    if complaints.is_empty() {
        Ok(())
    } else {
        Err(format!("not predictable enough to bake: {}", complaints.join("; ")))
    }
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

    /// A file whose own units are 16 fills the canvas exactly as the 24-unit original does.
    ///
    /// This is the whole point of reading the size from the file. Before, the same artwork at 16
    /// units baked at two thirds scale against the top-left corner, and the only way to get a full
    /// size icon was to wrap the artwork in `scale(1.5)` by hand.
    #[test]
    fn a_sixteen_unit_file_bakes_the_same_picture_as_a_twenty_four_unit_one() {
        let path = std::env::temp_dir().join("icon-bake-sixteen-units.svg");
        std::fs::write(
            &path,
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none"
                 stroke="#ffffff" stroke-width="1" stroke-linecap="round">
                 <path d="M4 8h8"/><path d="M8 4v8"/></svg>"##,
        )
        .unwrap();
        let sixteen = render_rgba(&path).unwrap();
        std::fs::write(
            &path,
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none"
                 stroke="#ffffff" stroke-width="1.5" stroke-linecap="round">
                 <path d="M6 12h12"/><path d="M12 6v12"/></svg>"##,
        )
        .unwrap();
        let twenty_four = render_rgba(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let ink = |pixels: &[u8]| pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert_eq!(
            ink(&sixteen),
            ink(&twenty_four),
            "16 units must fill the canvas the same as 24 units"
        );
        assert!(ink(&sixteen) > 100, "the cross should have drawn something");
    }

    /// SVG whose rendering would differ on another machine is refused, not baked.
    #[test]
    fn unpredictable_svg_is_refused() {
        let with_text = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><text x="2" y="12">hi</text></svg>"#;
        assert!(preflight(with_text).is_err(), "<text> must be refused");

        let with_link = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><image href="https://example.com/a.png"/></svg>"#;
        assert!(preflight(with_link).is_err(), "an external href must be refused");

        let with_filter = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M2 2h20v20H2z" filter="url(#f)"/></svg>"#;
        assert!(preflight(with_filter).is_err(), "a filter must be refused");

        let plain = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M2 2h20v20H2z"/></svg>"#;
        assert!(preflight(plain).is_ok(), "an ordinary path is fine");
    }
}
