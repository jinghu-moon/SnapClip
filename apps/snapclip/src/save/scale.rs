//! The size the panel asks for, applied to the pixels before they are encoded.
//!
//! The panel's size fields are the only place the output dimensions are decided, so a save that
//! ignored them would be a save of something other than what the panel showed. Two rules live
//! here, both pure, because both are arithmetic that a window would only make harder to assert:
//!
//! * [`output_size`] turns the two text fields into the size to write, falling back to the
//!   selection's own size when the text is not a usable number. The fallback is deliberate and
//!   logged by the caller: refusing to save because a field is half-typed would be worse than
//!   writing the selection as it stands.
//! * [`scaled`] resamples. **Area averaging when shrinking** (bilinear downsampling drops pixels
//!   and makes text shimmer) and **bilinear when growing** (area averaging degenerates into
//!   nearest neighbour, which leaves visible steps). Neither direction is allowed to change the
//!   alpha channel's meaning: it is resampled with the colours, not recomputed.
//!
//! The repository has two resamplers already, both inside the magnifier that draws the overlay's
//! zoom lens, and both are aimed at on-screen detail rather than at a final file. Reusing them
//! would tie a saving decision to a display decision, so this module is separate and tested on
//! its own.

/// A width and a height, in pixels, in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    fn pixels(self) -> usize {
        self.width as usize * self.height as usize
    }
}

/// The pixels a save was given and the size they must become.
///
/// Named fields rather than four positional arguments: the selection's size and the output size
/// are both a pair of `u32`, so an argument list would let one be passed for the other and only a
/// wrong-looking file would say so.
pub struct Frame<'a> {
    /// Tightly packed, top down, `bgra`, at `source`'s size.
    pub bgra: &'a [u8],
    /// What the capture handed over.
    pub source: Size,
    /// What the panel asks for.
    pub output: Size,
}

impl Frame<'_> {
    /// The same pixels at another output size.
    ///
    /// The shrink pass weighs candidate sizes; each is the same capture resampled differently, so
    /// this is the only thing that changes between attempts. Named rather than spelled out so a
    /// caller cannot forget one of the three fields and weigh a picture it will not write.
    pub fn at(&self, output: Size) -> Frame<'_> {
        Frame {
            bgra: self.bgra,
            source: self.source,
            output,
        }
    }
}

/// What the two size fields mean (`S.sizeMode`, `:734`).
///
/// The mockup keeps one pair of numbers and one mode, and switching the mode only changes how
/// those numbers are written down (`:1072`, `:1073`) — the size itself does not move. That is why
/// [`display_text`] exists next to [`output_size`]: one reads the fields, the other writes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeMode {
    /// The fields hold pixel counts (`数值`, `:585`).
    Pixels,
    /// The fields hold percentages of the capture (`百分比`, `:586`).
    Percent,
}

impl SizeMode {
    /// The mode a stored index names. Anything but the design's second button is pixels, so a
    /// state that was never set behaves like a fresh panel.
    pub const fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Percent,
            _ => Self::Pixels,
        }
    }
}

/// One field's text as a pixel count, or `None` when it is not a usable size.
///
/// Surrounding spaces are tolerated (the control is a text field a person types in), a leading
/// `+` is not, and zero is not a size.
pub fn parse_size(text: &str) -> Option<u32> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    trimmed.parse::<u32>().ok().filter(|value| *value > 0)
}

/// The size a save will write.
///
/// Both fields must be usable for the panel's answer to be taken; otherwise the selection's size
/// stands. A half-edited pair is not a request to scale to some other size.
///
/// In [`SizeMode::Percent`] each field is read as a share of the capture and the result is rounded
/// the way the mockup rounds it (`:1165`, `:1166`): `round(original * percent / 100)`. A share that
/// rounds to nothing becomes one pixel rather than an error, because a size of zero is the only
/// answer the rest of the save cannot use.
pub fn output_size(mode: SizeMode, width_text: &str, height_text: &str, source: Size) -> Size {
    let (width, height) = match (parse_size(width_text), parse_size(height_text)) {
        (Some(width), Some(height)) => (width, height),
        _ => return source,
    };
    match mode {
        SizeMode::Pixels => Size::new(width, height),
        SizeMode::Percent => Size::new(
            percent_of(source.width, width),
            percent_of(source.height, height),
        ),
    }
}

/// `round(base * percent / 100)`, at least one pixel.
fn percent_of(base: u32, percent: u32) -> u32 {
    let scaled = (u64::from(base) * u64::from(percent) + 50) / 100;
    scaled.max(1) as u32
}

/// The text the two fields should show for `size` in `mode`.
///
/// The mockup switches mode by rewriting these two fields from the size it already has (`:1072`,
/// `:1073`): `round(size / original * 100)` in percent, the pixel count otherwise. Returning text
/// rather than a pair of numbers keeps the caller from formatting them its own way.
pub fn display_text(mode: SizeMode, size: Size, source: Size) -> (String, String) {
    match mode {
        SizeMode::Pixels => (size.width.to_string(), size.height.to_string()),
        SizeMode::Percent => (
            share_percent(size.width, source.width).to_string(),
            share_percent(size.height, source.height).to_string(),
        ),
    }
}

/// `round(part / whole * 100)`, and zero when the whole is zero rather than a division error.
fn share_percent(part: u32, whole: u32) -> u32 {
    if whole == 0 {
        return 0;
    }
    ((u64::from(part) * 100 + u64::from(whole) / 2) / u64::from(whole)) as u32
}

/// `bgra` composited over `fill`, so nothing is left transparent.
///
/// JPEG has no alpha channel, so a transparent pixel would otherwise be encoded as whatever the
/// encoder makes of a colour that was never composited — in practice a dark halo. The capture's
/// pixels are **straight** alpha (`artifact.rs` spells that out), which is why the arithmetic
/// multiplies the colour by the alpha here rather than assuming that has already happened.
///
/// The result is the same layout with every alpha at `255`, because a JPEG has nowhere to put
/// anything else; pixels that were already opaque come back unchanged.
pub fn over_fill(bgra: &[u8], fill: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bgra.len());
    for pixel in bgra.chunks_exact(4) {
        let alpha = u32::from(pixel[3]);
        if alpha == 255 {
            out.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            continue;
        }
        let inverse = 255 - alpha;
        for channel in 0..3 {
            // `channel` indexes the buffer's own order (blue first), and the fill arrives in that
            // same order, so no channel is reordered here.
            let value =
                (u32::from(pixel[channel]) * alpha + u32::from(fill[channel]) * inverse + 127) / 255;
            out.push(value as u8);
        }
        out.push(255);
    }
    out
}

/// `frame.bgra` resampled to `frame.output`.
///
/// Two passes with an intermediate of the output width, so peak memory is one intermediate plus
/// the result rather than a weighted sum of every source pixel for every destination pixel.
pub fn scaled(frame: &Frame<'_>) -> Vec<u8> {
    if frame.source == frame.output {
        return frame.bgra.to_vec();
    }
    let source = frame.source;
    let output = frame.output;
    let source_width = source.width as usize;
    let output_width = output.width as usize;

    // Horizontal pass: every source row becomes an output-width row.
    let mut between = vec![0u8; output_width * source.height as usize * 4];
    let mut taps: Vec<(u32, f32)> = Vec::new();
    for y in 0..source.height as usize {
        let row = &frame.bgra[y * source_width * 4..(y + 1) * source_width * 4];
        for x in 0..output_width {
            axis_taps(source.width, output.width, x as u32, &mut taps);
            let mut sum = [0.0f32; 4];
            for &(index, weight) in taps.iter() {
                let pixel = &row[index as usize * 4..index as usize * 4 + 4];
                for channel in 0..4 {
                    sum[channel] += f32::from(pixel[channel]) * weight;
                }
            }
            let at = (y * output_width + x) * 4;
            for channel in 0..4 {
                between[at + channel] = round_channel(sum[channel]);
            }
        }
    }

    // Vertical pass: every column of the intermediate becomes an output-height column.
    let mut out = vec![0u8; output.pixels() * 4];
    for y in 0..output.height as usize {
        axis_taps(source.height, output.height, y as u32, &mut taps);
        for x in 0..output_width {
            let mut sum = [0.0f32; 4];
            for &(index, weight) in taps.iter() {
                let at = (index as usize * output_width + x) * 4;
                for channel in 0..4 {
                    sum[channel] += f32::from(between[at + channel]) * weight;
                }
            }
            let at = (y * output_width + x) * 4;
            for channel in 0..4 {
                out[at + channel] = round_channel(sum[channel]);
            }
        }
    }
    out
}

fn round_channel(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// Which source pixels one destination pixel on one axis is built from, and with what weight.
///
/// Shrinking averages: each destination pixel covers a span of the source, and every source pixel
/// it covers contributes in proportion to the overlap. Growing interpolates: the destination's
/// centre maps to a position between two source centres, and the two neighbours are mixed. The
/// weights always sum to one, so a flat source stays flat instead of drifting brighter or darker
/// over a long run of pixels.
fn axis_taps(source: u32, output: u32, index: u32, out: &mut Vec<(u32, f32)>) {
    out.clear();
    debug_assert!(source > 0 && output > 0);
    if output >= source {
        let ratio = source as f32 / output as f32;
        let position = (index as f32 + 0.5) * ratio - 0.5;
        let position = position.clamp(0.0, source as f32 - 1.0);
        let lower = position.floor() as u32;
        let upper = (lower + 1).min(source - 1);
        let fraction = position - lower as f32;
        if lower == upper {
            out.push((lower, 1.0));
        } else {
            out.push((lower, 1.0 - fraction));
            out.push((upper, fraction));
        }
        return;
    }
    let start = index as f32 * source as f32 / output as f32;
    let end = (index as f32 + 1.0) * source as f32 / output as f32;
    let span = end - start;
    let first = start.floor() as u32;
    let last = (end.ceil() as u32).min(source);
    for source_index in first..last {
        let low = (source_index as f32).max(start);
        let high = ((source_index + 1) as f32).min(end);
        let weight = (high - low) / span;
        if weight > 0.0 {
            out.push((source_index, weight));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A solid rectangle of one colour, so every pixel is checkable by value.
    fn solid(size: Size, bgra: [u8; 4]) -> Vec<u8> {
        bgra.iter()
            .copied()
            .cycle()
            .take(size.pixels() * 4)
            .collect()
    }

    fn pixel(buffer: &[u8], size: Size, x: u32, y: u32) -> [u8; 4] {
        let at = (y as usize * size.width as usize + x as usize) * 4;
        [buffer[at], buffer[at + 1], buffer[at + 2], buffer[at + 3]]
    }

    /// The same size is the same bytes: a save that asks for the selection's own size must not
    /// resample it and lose a level to rounding.
    #[test]
    fn a_frame_of_the_same_size_is_copied_byte_for_byte() {
        let size = Size::new(3, 2);
        let mut bgra = Vec::new();
        for index in 0..size.pixels() {
            bgra.extend_from_slice(&[index as u8, 7, 9, 255]);
        }
        let frame = Frame {
            bgra: &bgra,
            source: size,
            output: size,
        };
        assert_eq!(scaled(&frame), bgra);
    }

    /// Shrinking averages the pixels it covers rather than dropping three of every four.
    #[test]
    fn halving_averages_the_pixels_it_covers() {
        // 2x2 with four distinct colours, halved to one pixel: the average, not the corner.
        let bgra = vec![
            0, 0, 0, 255, // black
            100, 0, 0, 255, // red-ish (b=100)
            0, 100, 0, 255, // green-ish (g=100)
            100, 100, 0, 255, // both
        ];
        let frame = Frame {
            bgra: &bgra,
            source: Size::new(2, 2),
            output: Size::new(1, 1),
        };
        let out = scaled(&frame);
        assert_eq!(out, vec![50, 50, 0, 255], "the mean of the four, in BGRA order");
    }

    /// A flat colour stays flat at any size, which is what the weights summing to one buys.
    #[test]
    fn a_flat_colour_survives_shrinking_and_growing() {
        let source = Size::new(5, 3);
        let flat = solid(source, [40, 80, 120, 255]);
        for output in [Size::new(2, 2), Size::new(11, 7), Size::new(5, 3), Size::new(1, 1)] {
            let frame = Frame {
                bgra: &flat,
                source,
                output,
            };
            let out = scaled(&frame);
            assert_eq!(out.len(), output.pixels() * 4);
            for index in 0..output.pixels() {
                let at = index * 4;
                assert_eq!(
                    &out[at..at + 4],
                    &[40, 80, 120, 255],
                    "a flat source must stay flat at {output:?}"
                );
            }
        }
    }

    /// Growing keeps the corners of a two-colour source where they are, which is the cheapest
    /// visible check that the sampler is anchored to the source's edges and not to its centre.
    #[test]
    fn growing_keeps_the_corners() {
        let source = Size::new(2, 2);
        let bgra = vec![
            10, 10, 10, 255, 200, 200, 200, 255, //
            200, 200, 200, 255, 10, 10, 10, 255,
        ];
        let output = Size::new(4, 4);
        let frame = Frame {
            bgra: &bgra,
            source,
            output,
        };
        let out = scaled(&frame);
        assert_eq!(pixel(&out, output, 0, 0), [10, 10, 10, 255]);
        assert_eq!(pixel(&out, output, 3, 0), [200, 200, 200, 255]);
        assert_eq!(pixel(&out, output, 0, 3), [200, 200, 200, 255]);
        assert_eq!(pixel(&out, output, 3, 3), [10, 10, 10, 255]);
    }

    /// A ratio that does not divide evenly still uses every source column: 3 to 2 must not leave
    /// the third column unused, which a naive `index * source / output` sampler does.
    #[test]
    fn an_uneven_ratio_uses_the_last_column() {
        let source = Size::new(3, 1);
        let bgra = vec![
            0, 0, 0, 255, //
            0, 0, 0, 255, //
            250, 0, 0, 255,
        ];
        let output = Size::new(2, 1);
        let frame = Frame {
            bgra: &bgra,
            source,
            output,
        };
        let out = scaled(&frame);
        let right = pixel(&out, output, 1, 0)[0];
        assert!(
            right > 60,
            "the third column carries most of the second output pixel: {right}"
        );
    }

    /// Alpha is resampled with the colours rather than recomputed, so a translucent capture stays
    /// as translucent as it was.
    #[test]
    fn alpha_is_resampled_with_the_colours() {
        let source = Size::new(2, 1);
        let bgra = vec![255, 255, 255, 0, 255, 255, 255, 255];
        let output = Size::new(1, 1);
        let frame = Frame {
            bgra: &bgra,
            source,
            output,
        };
        assert_eq!(scaled(&frame), vec![255, 255, 255, 128], "the mean alpha");
    }

    #[test]
    fn a_size_field_is_read_as_pixels_when_it_holds_pixels() {
        assert_eq!(parse_size("1440"), Some(1440));
        assert_eq!(parse_size(" 1440 "), Some(1440));
        assert_eq!(parse_size("0"), None, "zero is not a size");
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("1440px"), None, "the unit is not part of the field");
        assert_eq!(parse_size("-3"), None);
        assert_eq!(parse_size("1 4"), None);
    }

    /// A pair of fields that is not usable falls back to the selection rather than to a guess.
    #[test]
    fn an_unusable_pair_keeps_the_selections_size() {
        let source = Size::new(1200, 800);
        assert_eq!(
            output_size(SizeMode::Pixels, "600", "400", source),
            Size::new(600, 400)
        );
        assert_eq!(
            output_size(SizeMode::Pixels, "600", "", source),
            source,
            "one field alone is not a size"
        );
        assert_eq!(output_size(SizeMode::Pixels, "abc", "400", source), source);
        assert_eq!(output_size(SizeMode::Pixels, "", "", source), source);
    }

    /// The same two numbers mean different sizes in the two modes, which is the whole point of the
    /// switch: `50` is a width in one and half the capture in the other.
    #[test]
    fn the_mode_decides_what_the_numbers_mean() {
        let source = Size::new(1200, 800);
        assert_eq!(
            output_size(SizeMode::Pixels, "600", "400", source),
            Size::new(600, 400)
        );
        assert_eq!(
            output_size(SizeMode::Percent, "600", "400", source),
            Size::new(7200, 3200),
            "600 percent of the capture, not 600 pixels"
        );
        assert_eq!(
            output_size(SizeMode::Percent, "50", "25", source),
            Size::new(600, 200)
        );
    }

    /// Percent rounds the way the mockup rounds (`:1165`), and a share that rounds to nothing is one
    /// pixel rather than an error the save cannot use.
    #[test]
    fn a_percentage_is_rounded_and_never_reaches_zero() {
        let source = Size::new(1441, 901);
        assert_eq!(
            output_size(SizeMode::Percent, "50", "50", source),
            Size::new(721, 451),
            "1441 / 2 rounds up, 901 / 2 rounds up"
        );
        assert_eq!(
            output_size(SizeMode::Percent, "1", "1", source),
            Size::new(14, 9),
            "one percent of these sizes rounds to 14 and 9"
        );
        assert_eq!(
            output_size(SizeMode::Percent, "1", "1", Size::new(30, 30)),
            Size::new(1, 1),
            "a share of a small capture still leaves a pixel"
        );
    }

    /// Switching the mode rewrites the two fields from the size that is already there, and leaves
    /// that size alone (`:1072`, `:1073`).
    #[test]
    fn switching_the_mode_only_rewrites_the_numbers() {
        let source = Size::new(1440, 900);
        let half = Size::new(720, 450);
        assert_eq!(
            display_text(SizeMode::Pixels, half, source),
            ("720".to_owned(), "450".to_owned())
        );
        assert_eq!(
            display_text(SizeMode::Percent, half, source),
            ("50".to_owned(), "50".to_owned())
        );
        assert_eq!(
            display_text(SizeMode::Percent, source, source),
            ("100".to_owned(), "100".to_owned()),
            "the capture itself is one hundred percent"
        );

        // The round trip the handler performs: read with the old mode, write with the new one.
        let size = output_size(SizeMode::Pixels, "720", "450", source);
        assert_eq!(size, half);
        assert_eq!(
            display_text(SizeMode::Percent, size, source),
            ("50".to_owned(), "50".to_owned())
        );
        let back = output_size(SizeMode::Percent, "50", "50", source);
        assert_eq!(back, half, "the size survives the round trip through percent");
    }

    /// A transparent pixel takes the fill colour; an opaque one is left exactly as it was.
    #[test]
    fn a_transparent_pixel_takes_the_fill_colour() {
        // Fully transparent, then fully opaque blue-ish.
        let pixels = [0u8, 0, 0, 0, 9, 8, 7, 255];
        assert_eq!(
            over_fill(&pixels, [0, 0, 0]),
            [0, 0, 0, 255, 9, 8, 7, 255],
            "transparent becomes black, opaque is untouched"
        );
        assert_eq!(
            over_fill(&pixels, [255, 255, 255]),
            [255, 255, 255, 255, 9, 8, 7, 255],
            "the fill is the chosen colour, not a constant"
        );

        // Half transparent, straight alpha: half the colour and half the fill.
        let half = [100u8, 50, 0, 128];
        let filled = over_fill(&half, [0, 0, 0]);
        assert_eq!(&filled[..3], &[50, 25, 0], "half of a straight-alpha colour");
        assert_eq!(filled[3], 255, "the result is opaque, because JPEG has no alpha");
    }
}
