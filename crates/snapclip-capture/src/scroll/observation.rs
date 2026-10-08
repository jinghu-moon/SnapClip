//! The axis abstraction: "vertical" and "horizontal" are the same problem instantiated twice
//! (`docs/30` §13.1, §17.8; first principles F-02/F-06), so the whole estimator is written once
//! against the primary axis and only the pixel movement forks.
//!
//! `Observation` (`P1.03`) lands in this file too: an observation carries the axis it was captured
//! on, so the two types share a home.

use crate::geometry::Rect;

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
    ///
    /// `pub(crate)` because the fixture (`testkit`) and the axis tests have to ask the same question
    /// — a second `match` there would be a second place for the axes to be told apart.
    pub(crate) const fn is_vertical(self) -> bool {
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

/// Why an observation could not be built.
///
/// Both variants are *input* errors: the frame source is supposed to hand over exactly one packed
/// viewport, so a mismatch here is a bug in the caller, not a condition to recover from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObservationError {
    /// `pixels.len()` is not `width * height * 4`.
    NotPacked { expected: usize, got: usize },
    /// The pixel size disagrees with the region's extent.
    SizeMismatch {
        size: (u32, u32),
        region: (u32, u32),
    },
}

impl core::fmt::Display for ObservationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ObservationError::NotPacked { expected, got } => write!(
                f,
                "an observation must be strictly packed: expected {expected} bytes, got {got}"
            ),
            ObservationError::SizeMismatch { size, region } => write!(
                f,
                "the pixel size {}x{} disagrees with the region's extent {}x{}",
                size.0, size.1, region.0, region.1
            ),
        }
    }
}

/// One viewport captured at one instant (`docs/30` §2.2).
///
/// The estimator's input unit, and **constructible without a window system** (§2.2, F1): that is
/// what makes the whole funnel testable on a synthetic fixture (`testkit.rs`).
///
/// Invariants, checked once in [`Observation::new`] and then relied on by every pixel index:
///
/// * `pixels` is strictly packed BGRA — `row_stride == width * 4`, no padding (`P1.03` REFACTOR);
/// * `size` and `region` agree about the viewport's extent (§28.2: the frame source crops before
///   it hands an observation over, so an observation never carries two sizes);
/// * an observation is immutable after construction: the estimator sees [`ObservationView`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Observation {
    pixels: Vec<u8>,
    region: Rect,
    qpc: i64,
    size: (u32, u32),
    axis: Axis,
}

// `P1.03` defines the surface in full because the invariants only mean something together;
// `P1.04` (`Displacement`) and `P1.05` (layer 1) are the first consumers, and the tests here use
// the subset they need. Same marker, same reason as `Axis` below.
#[allow(dead_code)]
impl Observation {
    /// `qpc` is the performance counter reading at capture time (`docs/30` §23.2 uses it for the
    /// stitch-latency measurement), `size` the viewport's pixel size.
    pub(crate) fn new(
        pixels: Vec<u8>,
        region: Rect,
        qpc: i64,
        size: (u32, u32),
        axis: Axis,
    ) -> Result<Self, ObservationError> {
        let expected = size.0 as usize * size.1 as usize * 4;
        if pixels.len() != expected {
            return Err(ObservationError::NotPacked {
                expected,
                got: pixels.len(),
            });
        }
        let region_size = (region.width() as u32, region.height() as u32);
        if region_size != size {
            return Err(ObservationError::SizeMismatch {
                size,
                region: region_size,
            });
        }
        Ok(Self {
            pixels,
            region,
            qpc,
            size,
            axis,
        })
    }

    pub(crate) fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub(crate) fn region(&self) -> Rect {
        self.region
    }

    pub(crate) fn qpc(&self) -> i64 {
        self.qpc
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        self.size
    }

    pub(crate) fn axis(&self) -> Axis {
        self.axis
    }

    pub(crate) fn width(&self) -> u32 {
        self.size.0
    }

    pub(crate) fn height(&self) -> u32 {
        self.size.1
    }

    /// Bytes per line. Always `width * 4` — there is no padding to account for (`P1.03` REFACTOR
    /// turns "strictly packed" from a comment into this single function plus its assertion).
    pub(crate) fn row_stride(&self) -> usize {
        self.size.0 as usize * 4
    }

    pub(crate) fn row(&self, y: u32) -> &[u8] {
        let stride = self.row_stride();
        let start = y as usize * stride;
        &self.pixels[start..start + stride]
    }

    /// The extent along the scrolling axis — the length `overlap_ratio` is taken over (§16.3).
    pub(crate) fn primary_extent(&self) -> u32 {
        self.axis.primary_extent(self.size.0, self.size.1)
    }

    /// The read-only handle the estimator is given (`docs/30` §16's `estimate` signature).
    pub(crate) fn view(&self) -> ObservationView<'_> {
        ObservationView::new(self)
    }
}

/// A borrowed, read-only observation — the **only** thing `estimate()` is allowed to see.
///
/// This is what makes "an estimator cannot modify an observation" a type-level fact instead of a
/// convention: the estimator receives `&ObservationView`, and every field behind it is a shared
/// reference to the owning [`Observation`].
///
/// `P1.05`+ hangs the derived, cacheable views here (grayscale lines, the 1/4 downsample, gradient
/// energy — §28.2 and §22.5's scratch reuse), which is why this is a struct rather than a
/// `&Observation` type alias.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ObservationView<'a> {
    observation: &'a Observation,
}

// The estimator-facing half of the same surface: `P1.05` is the first caller of most of these.
#[allow(dead_code)]
impl<'a> ObservationView<'a> {
    pub(crate) fn new(observation: &'a Observation) -> Self {
        Self { observation }
    }

    pub(crate) fn pixels(&self) -> &'a [u8] {
        self.observation.pixels()
    }

    pub(crate) fn region(&self) -> Rect {
        self.observation.region()
    }

    pub(crate) fn qpc(&self) -> i64 {
        self.observation.qpc()
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        self.observation.size()
    }

    pub(crate) fn axis(&self) -> Axis {
        self.observation.axis()
    }

    pub(crate) fn width(&self) -> u32 {
        self.observation.width()
    }

    pub(crate) fn height(&self) -> u32 {
        self.observation.height()
    }

    pub(crate) fn row_stride(&self) -> usize {
        self.observation.row_stride()
    }

    pub(crate) fn row(&self, y: u32) -> &'a [u8] {
        self.observation.row(y)
    }

    pub(crate) fn primary_extent(&self) -> u32 {
        self.observation.primary_extent()
    }
}

#[cfg(test)]
mod tests {
    use super::{Axis, Observation, ObservationView};

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
    use crate::scroll::testkit::{ScrollScript, StepSpec, TestImage};

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

    /// One fingerprint per line **of the primary axis**: rows for a vertical observation, columns
    /// for a horizontal one. The axis comes from the observation itself, so the two runs below go
    /// through this one function and the branch is the only place the axes are told apart.
    fn primary_fingerprints(frame: &Observation) -> Vec<u64> {
        if frame.axis().is_vertical() {
            frame
                .pixels()
                .chunks_exact(frame.row_stride())
                .map(fingerprint)
                .collect()
        } else {
            let (width, height) = (frame.width() as usize, frame.height() as usize);
            (0..width)
                .map(|x| {
                    let mut column = Vec::with_capacity(height * 4);
                    for y in 0..height {
                        let at = y * frame.row_stride() + x * 4;
                        column.extend_from_slice(&frame.pixels()[at..at + 4]);
                    }
                    fingerprint(&column)
                })
                .collect()
        }
    }

    /// `testkit`'s simplest estimator, generalised over the axis: largest agreement wins, smallest
    /// `|d|` breaks ties, and a candidate needs half the primary extent to overlap (`§16.5`). The
    /// axis is read off the observations, so both `T-AXIS-1` runs call exactly this code.
    fn recover_primary(previous: &Observation, current: &Observation, window: i32) -> Option<i32> {
        assert_eq!(previous.axis(), current.axis(), "one run, one axis");
        let a = primary_fingerprints(previous);
        let b = primary_fingerprints(current);
        assert_eq!(a.len(), b.len(), "both frames show one viewport");
        assert_eq!(
            a.len() as u32,
            previous.primary_extent(),
            "the number of signal lines is the primary extent, whatever the axis"
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

    /// `T-AXIS-1` (`P1.02`, kept green by `P1.03`): the same script, once as rows and once as the
    /// transposed input, answered by the same function. `testkit::transposed` does the rotation.
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
        let mut vertical_script = ScrollScript::new(&image, 900, steps.clone());
        let mut horizontal_script = ScrollScript::horizontal(&image, 900, steps.clone());
        assert_eq!(vertical_script.len(), horizontal_script.len());

        let vertical_frames: Vec<Observation> = (0..vertical_script.len())
            .map(|k| vertical_script.take(k))
            .collect();
        let horizontal_frames: Vec<Observation> = (0..horizontal_script.len())
            .map(|k| horizontal_script.take(k))
            .collect();

        // The byte-level statement, before any estimator runs: line `i` of a vertical frame *is*
        // line `i` of the horizontal one, because the horizontal script crops the transposed
        // document. If this fails, comparing the two runs would prove nothing about the axis.
        for (k, (vertical, horizontal)) in vertical_frames
            .iter()
            .zip(&horizontal_frames)
            .enumerate()
        {
            assert_eq!(vertical.axis(), Axis::Vertical);
            assert_eq!(horizontal.axis(), Axis::Horizontal);
            assert_eq!(
                primary_fingerprints(vertical),
                primary_fingerprints(horizontal),
                "frame {k}: a transposed document's columns are the original rows"
            );
            assert_eq!(
                vertical.primary_extent(),
                horizontal.primary_extent(),
                "frame {k}: both viewports are the same number of lines long"
            );
            assert_eq!(vertical.primary_extent(), 900);
        }

        // Run 1: rows of a 640x900 viewport. Run 2: columns of the transposed 900x640 viewport.
        // Both go through the same `recover_primary`.
        let recovered_vertical: Vec<Option<i32>> = (1..vertical_frames.len())
            .map(|k| recover_primary(&vertical_frames[k - 1], &vertical_frames[k], 700))
            .collect();
        let recovered_horizontal: Vec<Option<i32>> = (1..horizontal_frames.len())
            .map(|k| recover_primary(&horizontal_frames[k - 1], &horizontal_frames[k], 700))
            .collect();

        assert_eq!(
            recovered_vertical, recovered_horizontal,
            "the two axes disagree about the displacement of the same script"
        );

        for k in 1..vertical_frames.len() {
            let truth = steps[k - 1].delta;
            assert_eq!(recovered_vertical[k - 1], Some(truth), "vertical run, frame {k}");
            assert_eq!(
                recovered_horizontal[k - 1],
                Some(truth),
                "horizontal run, frame {k}"
            );

            // The recovered line shift *is* the primary component of the displacement, and a
            // scripted scroll has no drift across the axis.
            let (dx, dy) = (0, truth);
            assert_eq!(Axis::Vertical.primary_delta(dx, dy), truth);
            assert_eq!(Axis::Vertical.cross_delta(dx, dy), 0);
            let (dx, dy) = (truth, 0);
            assert_eq!(Axis::Horizontal.primary_delta(dx, dy), truth);
            assert_eq!(Axis::Horizontal.cross_delta(dx, dy), 0);
        }

        // Both viewports are 900 lines long, in their own coordinates: 640x900 seen vertically,
        // 900x640 seen horizontally.
        assert_eq!(Axis::Vertical.primary_extent(640, 900), 900);
        assert_eq!(Axis::Horizontal.primary_extent(900, 640), 900);
    }

    // --- `P1.03`: the observation and its read-only view ---------------------------------------

    /// `P1.03` RED (the name is kept from `docs/31` so the test can be traced): the geometry is
    /// **carried** on the observation and never inferred from the buffer length, and a buffer that
    /// is not exactly `w * h * 4` bytes is **rejected**.
    #[test]
    fn an_observation_keeps_its_geometry_and_ignores_extra_bytes() {
        let region = Rect::new(10, 20, 14, 23); // 4 x 3 pixels
        let packed = vec![0x7Fu8; 4 * 4 * 3];
        let observation = Observation::new(packed.clone(), region, 1_234, (4, 3), Axis::Vertical)
            .expect("a packed 4x3 BGRA buffer is a valid observation");

        // The geometry is carried, not derived from `packed.len()`.
        assert_eq!(observation.region(), region);
        assert_eq!(observation.size(), (4, 3));
        assert_eq!(observation.width(), 4);
        assert_eq!(observation.height(), 3);
        assert_eq!(observation.qpc(), 1_234);
        assert_eq!(observation.axis(), Axis::Vertical);
        assert_eq!(observation.pixels(), &packed[..]);

        // Strictly packed, no stride padding: the ZNCC index arithmetic assumes exactly this.
        assert_eq!(observation.row_stride(), 16);
        assert_eq!(observation.row(1), &packed[16..32]);

        // One pixel short and one pixel long are both rejected, not ignored.
        for bad in [4 * 4 * 3 - 4, 4 * 4 * 3 + 4] {
            assert!(
                Observation::new(vec![0u8; bad], region, 0, (4, 3), Axis::Vertical).is_err(),
                "a {bad}-byte buffer is not a 4x3 BGRA observation"
            );
        }
    }

    /// The read-only property is a type, not a convention: `estimate()` (`docs/30` §16) takes
    /// views, so an estimator can neither mutate the pixels nor take ownership of the observation.
    #[test]
    fn a_view_is_the_only_thing_an_estimator_can_see() {
        let observation = Observation::new(
            vec![0x11u8; 4 * 2 * 2],
            Rect::new(0, 0, 2, 2),
            7,
            (2, 2),
            Axis::Vertical,
        )
        .expect("packed 2x2");

        fn estimator_sees(prev: &ObservationView<'_>, next: &ObservationView<'_>) -> (u32, u32) {
            (prev.width() + prev.region().left as u32, next.height())
        }

        let view = ObservationView::new(&observation);
        assert_eq!(view.row(1), &observation.pixels()[8..16]);
        assert_eq!(estimator_sees(&view, &view), (2, 2));
        // `view` borrowed the observation: it is still usable here.
        assert_eq!(observation.pixels().len(), 16);
    }

    /// `P1.03` REFACTOR: "strictly packed" is an invariant that every pixel index in the funnel
    /// relies on, so it gets a test of its own instead of a comment.
    ///
    /// The producer side of this contract is the D3D11 readback, which strips `RowPitch` on the way
    /// out (`crates/snapclip-capture/src/windows/win/d3d11.rs:288`, `:366`, `:556-559`). If a future
    /// frame source ever hands a padded buffer straight through, this is the test that says so —
    /// rather than a wrong `d` produced by indexing with the wrong stride.
    #[test]
    fn a_row_stride_is_always_the_packed_one() {
        for (width, height) in [(1u32, 1u32), (3, 5), (640, 4), (1920, 2)] {
            let observation = Observation::new(
                vec![0u8; (width * height * 4) as usize],
                Rect::new(0, 0, width as i32, height as i32),
                0,
                (width, height),
                Axis::Vertical,
            )
            .expect("a packed buffer of exactly the right size");

            assert_eq!(observation.row_stride(), width as usize * 4);
            assert_eq!(
                observation.pixels().len(),
                observation.row_stride() * height as usize
            );
            // The last row ends exactly at the end of the buffer: no trailing padding either.
            assert_eq!(
                observation.row(height - 1).as_ptr_range().end,
                observation.pixels().as_ptr_range().end
            );
        }

        // A D3D11-style padded row pitch is not an observation: 3 px packed is 12 bytes/row, so a
        // 16-byte pitch belongs to a layout this type does not speak.
        let padded = vec![0u8; 16 * 5];
        assert!(Observation::new(padded, Rect::new(0, 0, 3, 5), 0, (3, 5), Axis::Vertical).is_err());
    }
}
