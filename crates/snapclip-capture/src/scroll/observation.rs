//! The axis abstraction: "vertical" and "horizontal" are the same problem instantiated twice
//! (`docs/30` §13.1, §17.8; first principles F-02/F-06), so the whole estimator is written once
//! against the primary axis and only the pixel movement forks.
//!
//! `Observation` (`P1.03`) lands in this file too: an observation carries the axis it was captured
//! on, so the two types share a home.

/// Which way the captured content moves across the viewport (`docs/30` §13.1).
///
/// Every estimator in `scroll/` is written once and parameterised by this; only the pixel movement
/// forks (§17.8, `N6`).
// `P1.02` defines the axis before anything consumes it; `P1.05` (layer 1) is the first consumer.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Axis {
    /// The content moves along the viewport's height; the primary component is `dy`.
    Vertical,
    /// The content moves along the viewport's width; the primary component is `dx`.
    Horizontal,
}

#[allow(dead_code)]
impl Axis {
    /// The single dispatch point (`docs/30` §17.8: "the only place the axis forks, and the type
    /// system will not catch it").
    ///
    /// Keeping the two axes apart as *one bit* is deliberate: it is what makes `Axis::Vertical`
    /// appear exactly once in this file, so the exit condition "at most one axis branch" is
    /// checkable with `grep` instead of by reading three `match`es.
    const fn is_vertical(self) -> bool {
        match self {
            Axis::Vertical => true,
            Axis::Horizontal => false,
        }
    }

    /// The displacement component along the scrolling direction.
    pub(crate) const fn primary_delta(self, dx: i32, dy: i32) -> i32 {
        if self.is_vertical() { dy } else { dx }
    }

    /// The displacement component across the scrolling direction.
    ///
    /// Measured and reported, **never compensated** (`docs/30` §13.1, §14.4).
    pub(crate) const fn cross_delta(self, dx: i32, dy: i32) -> i32 {
        if self.is_vertical() { dx } else { dy }
    }

    /// The image extent along the scrolling direction (the object `overlap_ratio` is taken over,
    /// §16.3).
    pub(crate) const fn primary_extent(self, width: u32, height: u32) -> u32 {
        if self.is_vertical() { height } else { width }
    }
}

#[cfg(test)]
mod tests {
    use super::Axis;

    /// `T-AXIS-1`'s mapping table, written by hand so the implementation cannot define its own
    /// truth. Rows 2 and 3 are the ones that get written backwards — `cross_delta` takes `dx` on
    /// the vertical axis and `dy` on the horizontal one (`docs/30` §13.1; §17.8's note calls it
    /// "the only place the axis forks, and the type system will not catch it").
    ///
    /// RED record (`2026-10-08`): this test did not compile — `error[E0432]: unresolved import
    /// `super::Axis``.
    #[test]
    fn the_axis_mapping_is_exhaustive() {
        // A displacement that is not symmetric in `dx`/`dy`, so a swapped mapping cannot pass by
        // accident.
        let (dx, dy) = (3, -7);

        // (1) displacement on the vertical axis: the primary component is `dy`.
        assert_eq!(Axis::Vertical.primary_delta(dx, dy), dy);
        assert_eq!(Axis::Vertical.cross_delta(dx, dy), dx);

        // (2) displacement on the horizontal axis: the primary component is `dx`.
        assert_eq!(Axis::Horizontal.primary_delta(dx, dy), dx);
        assert_eq!(Axis::Horizontal.cross_delta(dx, dy), dy);

        // (3) extent on the vertical axis: the primary extent is the height.
        assert_eq!(Axis::Vertical.primary_extent(640, 900), 900);

        // (4) extent on the horizontal axis: the primary extent is the width.
        assert_eq!(Axis::Horizontal.primary_extent(640, 900), 640);
    }

    // --- `T-AXIS-1` -------------------------------------------------------------------------
    //
    // Same script, both axes, answers compared line by line (`docs/30` §17.9 last row, §30).
    //
    // The estimator here is deliberately the dumb one from `P1.01`'s fixture proof (per-line
    // fingerprints, no downsampling, no confidence, no gate): the *only* thing this test varies is
    // the axis, so any difference in the answers can only come from the mapping.

    use crate::geometry::Rect;
    use crate::scroll::testkit::{ScrollScript, StepSpec, TestFrame, TestImage};

    /// FNV-1a over one line. Same constants as `testkit`'s fingerprints — duplicated rather than
    /// shared because those live inside that file's own `#[cfg(test)] mod tests`.
    fn fingerprint(bytes: &[u8]) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for byte in bytes {
            h ^= u64::from(*byte);
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        h
    }

    /// One fingerprint per line **of the primary axis**: rows for the vertical axis, columns for
    /// the horizontal one. Written with `is_vertical()` rather than a `match` so the axis is still
    /// told apart in exactly one place in this file.
    fn primary_fingerprints(frame: &TestFrame, axis: Axis) -> Vec<u64> {
        let (width, height) = (frame.width() as usize, frame.height() as usize);
        if axis.is_vertical() {
            frame
                .pixels
                .chunks_exact(width * 4)
                .map(fingerprint)
                .collect()
        } else {
            (0..width)
                .map(|x| {
                    let mut column = Vec::with_capacity(height * 4);
                    for y in 0..height {
                        let at = (y * width + x) * 4;
                        column.extend_from_slice(&frame.pixels[at..at + 4]);
                    }
                    fingerprint(&column)
                })
                .collect()
        }
    }

    /// `testkit`'s simplest estimator, generalised over the axis: largest agreement wins, smallest
    /// `|d|` breaks ties, and a candidate needs half the primary extent to overlap (`§16.5`).
    fn recover_primary(
        previous: &TestFrame,
        current: &TestFrame,
        axis: Axis,
        window: i32,
    ) -> Option<i32> {
        let a = primary_fingerprints(previous, axis);
        let b = primary_fingerprints(current, axis);
        assert_eq!(a.len(), b.len(), "both frames show one viewport");
        assert_eq!(
            a.len() as u32,
            axis.primary_extent(previous.width(), previous.height()),
            "the number of lines is the primary extent, whatever the axis"
        );
        let extent = a.len() as i32;

        let mut best: Option<(usize, i32)> = None;
        for d in -window..=window {
            if (extent - d.abs()) * 2 < extent {
                continue;
            }
            let matches = (0..extent)
                .filter(|y| {
                    let j = y + d;
                    (0..extent).contains(&j) && b[*y as usize] == a[j as usize]
                })
                .count();
            let better = match best {
                None => true,
                Some((best_matches, best_d)) => {
                    matches > best_matches || (matches == best_matches && d.abs() < best_d.abs())
                }
            };
            if better {
                best = Some((matches, d));
            }
        }
        best.map(|(_, d)| d)
    }

    /// The transposed input: rotate the frame 90°, so what scrolled along the document's rows now
    /// scrolls along this image's rows.
    ///
    /// BGRA is four bytes per pixel, so a transposition moves 4-byte groups; the round trip is
    /// asserted by the caller.
    fn transposed(frame: &TestFrame) -> TestFrame {
        let (width, height) = (frame.width() as usize, frame.height() as usize);
        let mut pixels = vec![0u8; frame.pixels.len()];
        for y in 0..height {
            for x in 0..width {
                let from = (y * width + x) * 4;
                let to = (x * height + y) * 4;
                pixels[to..to + 4].copy_from_slice(&frame.pixels[from..from + 4]);
            }
        }
        TestFrame {
            pixels,
            region: Rect::new(0, 0, height as i32, width as i32),
        }
    }

    #[test]
    fn the_same_script_answers_the_same_on_both_axes() {
        let image = TestImage::synthetic(640, 4_000, 0x5EED);
        let steps = vec![
            StepSpec::move_by(120),
            StepSpec::move_by(-120),
            StepSpec::move_by(37),
            StepSpec::repeat(),
            StepSpec::move_by(240),
        ];
        let mut script = ScrollScript::new(&image, 900, steps.clone());
        let frames: Vec<TestFrame> = (0..script.len()).map(|k| script.take(k)).collect();

        // Run 1: rows of a 640x900 viewport.
        let vertical: Vec<Option<i32>> = (1..frames.len())
            .map(|k| recover_primary(&frames[k - 1], &frames[k], Axis::Vertical, 700))
            .collect();

        // Run 2: the same script with every frame transposed. The estimator above is literally the
        // same function; `Axis::Horizontal` only decides that the lines it walks are columns.
        let flipped: Vec<TestFrame> = frames.iter().map(transposed).collect();
        for (k, frame) in frames.iter().enumerate() {
            assert_eq!(
                transposed(&flipped[k]).pixels, frame.pixels,
                "the transposition must be exact for the comparison below to mean anything"
            );
        }
        let horizontal: Vec<Option<i32>> = (1..flipped.len())
            .map(|k| recover_primary(&flipped[k - 1], &flipped[k], Axis::Horizontal, 700))
            .collect();

        assert_eq!(
            vertical, horizontal,
            "the two axes disagree about the displacement of the same script"
        );

        for k in 1..frames.len() {
            let truth = steps[k - 1].delta;
            assert_eq!(vertical[k - 1], Some(truth), "vertical run, frame {k}");
            assert_eq!(horizontal[k - 1], Some(truth), "horizontal run, frame {k}");

            // The recovered line shift *is* the primary component of the displacement, and a
            // scripted scroll has no drift across the axis.
            let (dx, dy) = (0, truth);
            assert_eq!(Axis::Vertical.primary_delta(dx, dy), truth);
            assert_eq!(Axis::Vertical.cross_delta(dx, dy), 0);
            let (dx, dy) = (truth, 0);
            assert_eq!(Axis::Horizontal.primary_delta(dx, dy), truth);
            assert_eq!(Axis::Horizontal.cross_delta(dx, dy), 0);

            // The physical length the overlap rule is taken over is the same on both axes.
            assert_eq!(
                Axis::Vertical.primary_extent(frames[k].width(), frames[k].height()),
                Axis::Horizontal.primary_extent(flipped[k].width(), flipped[k].height()),
            );
        }
    }
}
