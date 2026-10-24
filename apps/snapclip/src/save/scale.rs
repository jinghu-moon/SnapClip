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
pub fn output_size(width_text: &str, height_text: &str, source: Size) -> Size {
    match (parse_size(width_text), parse_size(height_text)) {
        (Some(width), Some(height)) => Size::new(width, height),
        _ => source,
    }
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
        assert_eq!(output_size("600", "400", source), Size::new(600, 400));
        assert_eq!(output_size("600", "", source), source, "one field alone is not a size");
        assert_eq!(output_size("abc", "400", source), source);
        assert_eq!(output_size("", "", source), source);
    }
}
