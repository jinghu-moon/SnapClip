//! `docs/31` `P1.01`: the synthetic fixture every other `P1` task is judged against.
//!
//! First principles **F-03/F-04** — the *unknown displacement* is the problem, so only an input
//! whose displacement is known can decide whether an estimator is right. `docs/30` §11.2 puts
//! the fixture before the algorithm for that reason.
//!
//! **`DEV-1`**: this file is the eleventh file of `scroll/` and exists only under
//! `#[cfg(test)]` (registered by `scroll/mod.rs`). `docs/30` §28.2's list stopped at ten.
//!
//! # What the fixture promises
//!
//! * **Deterministic**: no clock, no ambient entropy. The only entropy is the caller's `seed`,
//!   so two runs on two machines produce the same bytes.
//! * **Known truth**: the script decides the document offset of every viewport, so `truth(k)`
//!   and `viewport_rect(k)` are inputs to the test, not results of it.
//! * **Seven structures**: large flat areas, a row-periodic carrier, a two-dimensional
//!   checkerboard, a smooth gradient, noise blocks, text-like high-frequency rows and 1 px
//!   hairlines — the cases `docs/30` §29.3 names.
//! * **Scriptable transitions**: positive/negative steps, repeats, jumps, a moving (sticky)
//!   region and per-frame noise, so the failure modes of §18.2 can be produced on demand.
//!
//! # Why the frames are not `Observation` yet
//!
//! The task's intended surface is `fn take(&mut self, k: usize) -> Observation`, and
//! `Observation` is `P1.03`'s deliverable. `P1.01` therefore hands out its own frame type and
//! `P1.03` turns it into the real one; inventing the real one here would make `P1.03`'s RED
//! (its type-construction tests) pass before it is written.

// The fixture is an API for the whole `P1` sweep (`P1.05`…`P1.24`); each task uses a subset, so
// items that no test in *this* task touches are expected rather than forgotten.
#![allow(dead_code)]

use snapclip_model::geometry::Rect;

/// Rows per structure band in the default document. Small enough that a single viewport sees
/// several structures, large enough that a step of a few hundred rows still lands inside one.
pub(crate) const BAND_HEIGHT: u32 = 240;

/// The seven structures of `docs/30` §29.3, in the order the default document cycles them.
pub(crate) static STRUCTURES: [Structure; 7] = [
    Structure::Flat,
    // A `period`-row carrier: the axis it is periodic along is the axis that can be fooled by it
    // (`P1.05`'s false-peak case, `docs/30` §15.4).
    Structure::HorizontalBars { period: 37 },
    Structure::Checker { cell: 24 },
    Structure::Gradient,
    Structure::NoiseBlocks { cell: 8 },
    // Text line pitch is the carrier a real page has: 19 px is not a divisor of a wheel step.
    Structure::TextRows { line: 19 },
    Structure::Hairlines { step: 40 },
];

/// One texture. `level` returns a grey level; the fixture writes it to B, G and R, because
/// nothing in `scroll/` may depend on channel order (the estimator takes luma).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Structure {
    /// A large flat area: no gradient anywhere, so a matcher has nothing to lock onto.
    Flat,
    /// Horizontal bars of `period` rows: structure along the vertical axis only.
    HorizontalBars { period: u32 },
    /// A two-dimensional board — the structure that makes a *wrong* vertical shift look right
    /// when the cell size divides the shift.
    Checker { cell: u32 },
    /// A smooth gradient: structure everywhere, edges nowhere.
    Gradient,
    /// Blocks of hashed noise, i.e. the one structure with genuine high-frequency content.
    NoiseBlocks { cell: u32 },
    /// Rows of dark dashes on a light page, pitched every `line` rows.
    TextRows { line: u32 },
    /// 1 px dark lines every `step` rows, plus vertical hairlines that are invisible to a
    /// vertical search and must stay invisible (`P1.02`'s `T-AXIS-1`).
    Hairlines { step: u32 },
}

impl Structure {
    fn level(&self, ctx: Level) -> u8 {
        match *self {
            Structure::Flat => (82 + (ctx.band * 37) % 5 * 34) as u8,
            Structure::HorizontalBars { period } => {
                if (ctx.y % period.max(1)) * 2 < period.max(1) {
                    44
                } else {
                    208
                }
            }
            Structure::Checker { cell } => {
                let cell = cell.max(1);
                if ((ctx.x / cell) + (ctx.y / cell)) % 2 == 0 {
                    232
                } else {
                    64
                }
            }
            Structure::Gradient => {
                let g = ctx.x * 180 / ctx.width.max(1) + ctx.y * 70 / ctx.band_height.max(1);
                g.min(255) as u8
            }
            Structure::NoiseBlocks { cell } => {
                (hash(ctx.x / cell.max(1), ctx.y / cell.max(1), ctx.seed ^ 0x11A3) & 0xFF) as u8
            }
            Structure::TextRows { line } => {
                let line = line.max(1);
                let row = ctx.y % line;
                let in_glyph = (2..15).contains(&row);
                let dark = in_glyph && ctx.x >= 24 && hash(ctx.x / 6, ctx.y / line, ctx.seed ^ 0x7E47) % 6 < 2;
                if dark { 32 } else { 246 }
            }
            Structure::Hairlines { step } => {
                if ctx.y % step.max(1) == 0 {
                    28
                } else if ctx.x % 211 == 0 {
                    60
                } else {
                    242
                }
            }
        }
    }
}

struct Level {
    x: u32,
    y: u32,
    width: u32,
    band_height: u32,
    band: u32,
    seed: u32,
}

/// The synthetic document. Its bytes are a pure function of `(width, height, seed, structures)`.
#[derive(Debug, Clone)]
pub(crate) struct TestImage {
    width: u32,
    height: u32,
    band_height: u32,
    structures: Vec<Structure>,
    pixels: Vec<u8>,
}

impl TestImage {
    /// The default document: `STRUCTURES` cycling every `BAND_HEIGHT` rows.
    pub(crate) fn synthetic(width: u32, height: u32, seed: u32) -> Self {
        Self::from_structures(width, height, seed, BAND_HEIGHT, &STRUCTURES)
    }

    /// A document whose bands are the caller's structures, cycling in order. `P1.24`'s sweep
    /// needs single-structure documents; everything else can use `synthetic`.
    pub(crate) fn from_structures(
        width: u32,
        height: u32,
        seed: u32,
        band_height: u32,
        structures: &[Structure],
    ) -> Self {
        assert!(width > 0 && height > 0, "a document needs a positive size");
        assert!(band_height > 0, "bands cannot be empty");
        assert!(!structures.is_empty(), "a document needs at least one structure");

        let stride = width as usize * 4;
        let mut pixels = vec![0u8; stride * height as usize];
        for y in 0..height {
            let band = y / band_height;
            let structure = structures[(band as usize) % structures.len()];
            let ctx = Level {
                x: 0,
                y: y % band_height,
                width,
                band_height,
                band,
                seed,
            };
            let row = y as usize * stride;
            for x in 0..width {
                let level = structure.level(Level { x, ..ctx });
                let at = row + x as usize * 4;
                pixels[at] = level;
                pixels[at + 1] = level;
                pixels[at + 2] = level;
                pixels[at + 3] = 0xFF;
            }
        }
        Self {
            width,
            height,
            band_height,
            structures: structures.to_vec(),
            pixels,
        }
    }

    pub(crate) fn width(&self) -> u32 {
        self.width
    }

    pub(crate) fn height(&self) -> u32 {
        self.height
    }

    pub(crate) fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// One packed BGRA row, `width * 4` bytes.
    pub(crate) fn row(&self, y: u32) -> &[u8] {
        let stride = self.width as usize * 4;
        let start = y as usize * stride;
        &self.pixels[start..start + stride]
    }

    pub(crate) fn structure_at(&self, y: u32) -> Structure {
        self.structures[(y / self.band_height) as usize % self.structures.len()]
    }

    /// A packed BGRA copy of `region`, no stride padding (`P1.03` will assert `stride == w * 4`).
    fn crop(&self, region: Rect) -> Vec<u8> {
        let stride = self.width as usize * 4;
        let width = region.width() as usize;
        let mut out = Vec::with_capacity(width * region.height() as usize);
        for y in region.top..region.bottom {
            let start = y as usize * stride + region.left as usize * 4;
            out.extend_from_slice(&self.pixels[start..start + width * 4]);
        }
        out
    }
}

/// One step of a script: how far the document moves, and what the frame adds on top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StepSpec {
    /// Document rows the viewport moves **down** by. Negative = scrolling back up. This is the
    /// value `ScrollScript::truth` reports.
    pub delta: i32,
    /// Fraction of the viewport height (measured from the top) replaced by content that changes
    /// every frame — a sticky header, an animated banner, a video. `0.0` = a still page.
    pub dynamic: f32,
    /// Per-pixel noise amplitude, applied to every channel. `0` = a lossless frame.
    pub noise: u8,
}

impl StepSpec {
    pub(crate) fn move_by(delta: i32) -> Self {
        Self {
            delta,
            dynamic: 0.0,
            noise: 0,
        }
    }

    /// The same frame twice: the page was already at rest, or the wheel landed on a sticky
    /// element. `docs/30` §18.2's "no movement" case, which must not be a `Confirmed` step of 0
    /// by accident.
    pub(crate) fn repeat() -> Self {
        Self::move_by(0)
    }

    /// A step larger than the viewport overlap, so the two frames share nothing (`Home`/`End`,
    /// a scrollbar drag, a wheel burst). Mechanically this is `move_by`; the name is what makes
    /// a script readable, and the case is what §16.5's `Contained`/`Lost` paths exist for.
    pub(crate) fn jump_to(delta: i32) -> Self {
        Self::move_by(delta)
    }

    pub(crate) fn with_dynamic(mut self, ratio: f32) -> Self {
        assert!(
            (0.0..=1.0).contains(&ratio),
            "the moving region is a fraction of the viewport"
        );
        self.dynamic = ratio;
        self
    }

    pub(crate) fn with_noise(mut self, sigma: u8) -> Self {
        self.noise = sigma;
        self
    }
}

/// One frame of a script. Stands in for `Observation` until `P1.03` defines it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TestFrame {
    pub pixels: Vec<u8>,
    pub region: Rect,
}

impl TestFrame {
    pub(crate) fn width(&self) -> u32 {
        self.region.width() as u32
    }

    pub(crate) fn height(&self) -> u32 {
        self.region.height() as u32
    }
}

/// A scripted scroll over one [`TestImage`].
///
/// The viewport is a full-width rectangle at `x = 0`, because the horizontal axis arrives with
/// `P1.02`'s transposed input rather than with a second dimension here.
pub(crate) struct ScrollScript {
    image: TestImage,
    viewport_height: u32,
    steps: Vec<StepSpec>,
    offsets: Vec<i32>,
    cursor: usize,
}

impl ScrollScript {
    /// Frames = the initial viewport plus one per step. The script's offsets are computed here,
    /// once, and asserted to stay inside the document: a fixture that silently produced a
    /// partially out-of-document frame would report defeats that belong to nobody's algorithm.
    pub(crate) fn new(image: &TestImage, viewport_height: u32, steps: Vec<StepSpec>) -> Self {
        assert!(
            viewport_height > 0 && viewport_height <= image.height(),
            "the viewport must fit inside the document"
        );
        let mut offsets = vec![0i32];
        for step in &steps {
            let next = offsets[offsets.len() - 1] + step.delta;
            assert!(
                next >= 0 && next as u32 + viewport_height <= image.height(),
                "step {} leaves the document: offset {} + viewport {} > height {}",
                step.delta,
                next,
                viewport_height,
                image.height()
            );
            offsets.push(next);
        }
        Self {
            image: image.clone(),
            viewport_height,
            steps,
            offsets,
            cursor: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.steps.len() + 1
    }

    /// The displacement the step into frame `k` applied; `0` for the first frame, which no step
    /// produced.
    pub(crate) fn truth(&self, k: usize) -> i32 {
        if k == 0 { 0 } else { self.steps[k - 1].delta }
    }

    pub(crate) fn viewport_rect(&self, k: usize) -> Rect {
        let top = self.offsets[k];
        Rect::new(
            0,
            top,
            self.image.width() as i32,
            top + self.viewport_height as i32,
        )
    }

    pub(crate) fn viewport_height(&self) -> u32 {
        self.viewport_height
    }

    pub(crate) fn image(&self) -> &TestImage {
        &self.image
    }

    /// Frames are handed out in order. That is the whole point: a session sees a stream, and a
    /// test that could ask for frame 7 before frame 6 would not be testing a stream.
    pub(crate) fn take(&mut self, k: usize) -> TestFrame {
        assert_eq!(
            k, self.cursor,
            "take() is sequential: the fixture hands out the stream, not a random access"
        );
        self.cursor += 1;

        let region = self.viewport_rect(k);
        let mut pixels = self.image.crop(region);
        if k > 0 {
            let step = self.steps[k - 1];
            if step.dynamic > 0.0 {
                overlay_moving_region(
                    &mut pixels,
                    self.image.width(),
                    self.viewport_height,
                    step.dynamic,
                    k as u32,
                );
            }
            if step.noise > 0 {
                add_noise(
                    &mut pixels,
                    self.image.width(),
                    self.viewport_height,
                    step.noise,
                    k as u32,
                );
            }
        }
        TestFrame { pixels, region }
    }
}

/// Replace the top `ratio` of the frame with content keyed by the frame index, i.e. something
/// that changes even when the page does not move.
fn overlay_moving_region(pixels: &mut [u8], width: u32, height: u32, ratio: f32, frame: u32) {
    let band = ((height as f32 * ratio).round() as u32).clamp(1, height);
    let stride = width as usize * 4;
    for y in 0..band {
        for x in 0..width {
            let level = (hash(x / 3, y / 3, 0x5A17 ^ frame) & 0xFF) as u8;
            let at = y as usize * stride + x as usize * 4;
            pixels[at] = level;
            pixels[at + 1] = level;
            pixels[at + 2] = level;
        }
    }
}

/// Add deterministic noise of amplitude `sigma`, keyed by the frame index.
fn add_noise(pixels: &mut [u8], width: u32, height: u32, sigma: u8, frame: u32) {
    let stride = width as usize * 4;
    let sigma = sigma as i32;
    for y in 0..height {
        for x in 0..width {
            let n = (hash(x, y, 0x2F19 ^ frame) % (2 * sigma as u32 + 1)) as i32 - sigma;
            let at = y as usize * stride + x as usize * 4;
            for channel in 0..3 {
                pixels[at + channel] = (pixels[at + channel] as i32 + n).clamp(0, 255) as u8;
            }
        }
    }
}

/// A small integer hash. Not a cryptographic one: it only has to be seed-sensitive, cheap and
/// identical on every machine.
fn hash(a: u32, b: u32, salt: u32) -> u32 {
    let mut h = a
        .wrapping_mul(0x9E37_79B1)
        ^ b.wrapping_mul(0x85EB_CA6B)
        ^ salt.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^ (h >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One script: a step, a repeat, a step back, a jump, and a step that is both noisy and
    /// has a moving region. Nothing here needs to be clever — it only has to name every
    /// transition the fixture must support.
    fn a_script() -> Vec<StepSpec> {
        vec![
            StepSpec::move_by(120),
            StepSpec::move_by(120),
            StepSpec::repeat(),
            StepSpec::move_by(-120),
            StepSpec::jump_to(360),
            StepSpec::move_by(90).with_dynamic(0.25).with_noise(4),
        ]
    }

    #[test]
    fn the_fixture_reports_the_script_it_was_built_from() {
        let image = TestImage::synthetic(640, 4_000, 0x5EED);
        let steps = a_script();
        let mut script = ScrollScript::new(&image, 900, steps.clone());

        // Frames = the initial viewport plus one per step.
        assert_eq!(script.len(), steps.len() + 1);

        // The test recomputes the offset itself: if the fixture and the test agreed on a wrong
        // number, this assertion could not tell.
        let mut offset = 0i32;
        let mut offsets = vec![0i32];
        for step in &steps {
            offset += step.delta;
            offsets.push(offset);
        }

        for k in 0..script.len() {
            let expected_delta = if k == 0 { 0 } else { steps[k - 1].delta };
            assert_eq!(script.truth(k), expected_delta, "truth({k})");

            let expected_rect = Rect::new(0, offsets[k], 640, offsets[k] + 900);
            assert_eq!(script.viewport_rect(k), expected_rect, "viewport_rect({k})");

            let frame = script.take(k);
            assert_eq!(frame.region, expected_rect, "frame {k} region");
            assert_eq!(frame.width(), 640);
            assert_eq!(frame.height(), 900);
            assert_eq!(
                frame.pixels.len(),
                640 * 900 * 4,
                "frames are packed BGRA with no stride padding (ZNCC indexes rows by width * 4)"
            );
            // Pure-translation steps must be byte-exact crops of the document. The last step is
            // noisy and carries a moving region, so it is checked by its geometry only.
            if k < steps.len() {
                assert_eq!(
                    &frame.pixels[..640 * 4],
                    image.row(offsets[k] as u32),
                    "frame {k} row 0 must be the document row under it"
                );
                assert_eq!(
                    &frame.pixels[640 * 4 * 899..],
                    image.row((offsets[k] + 899) as u32),
                    "frame {k} last row"
                );
            }
        }
    }

    #[test]
    fn the_fixture_has_no_entropy_outside_its_seed() {
        let a = TestImage::synthetic(320, 1_200, 7);
        let b = TestImage::synthetic(320, 1_200, 7);
        let c = TestImage::synthetic(320, 1_200, 8);
        assert_eq!(a.pixels(), b.pixels(), "same seed must give the same bytes");
        assert_ne!(
            a.pixels(),
            c.pixels(),
            "a different seed must give different bytes"
        );

        let steps = vec![StepSpec::move_by(48).with_dynamic(0.5).with_noise(12); 4];
        let mut first = ScrollScript::new(&a, 400, steps.clone());
        let mut second = ScrollScript::new(&a, 400, steps);
        for k in 0..first.len() {
            assert_eq!(
                first.take(k).pixels,
                second.take(k).pixels,
                "frame {k} must not depend on when it is produced"
            );
        }

        // ...and the moving region / noise must be real, not a parameter nobody applies. Frame 2
        // is the one step 1 produced, so that is where a step's decoration has to show up.
        let steps = |step: StepSpec| vec![StepSpec::move_by(48), step];
        let mut plain = ScrollScript::new(&a, 400, steps(StepSpec::move_by(48)));
        let mut moving = ScrollScript::new(&a, 400, steps(StepSpec::move_by(48).with_dynamic(0.5)));
        let mut noisy = ScrollScript::new(&a, 400, steps(StepSpec::move_by(48).with_noise(12)));
        for script in [&mut plain, &mut moving, &mut noisy] {
            let _ = script.take(0);
            let _ = script.take(1);
        }
        let plain = plain.take(2).pixels;
        let moving = moving.take(2).pixels;
        let noisy = noisy.take(2).pixels;

        let stride = 320 * 4;
        let at = |row: usize| row * stride;
        let differing = |a: &[u8], b: &[u8], from: usize| {
            a[at(from)..].iter().zip(&b[at(from)..]).filter(|(x, y)| x != y).count()
        };

        // A moving region rewrites the top half and must leave the rest untouched — if it leaked
        // downwards, every later estimator test would be measuring the overlay, not the scroll.
        assert!(
            differing(&plain, &moving, 0) > 0,
            "a moving region must change the top of the frame"
        );
        assert_eq!(
            differing(&plain, &moving, 200),
            0,
            "a moving region covering 50% of the viewport must stop at row 200"
        );
        // Noise is everywhere, so a row near the bottom must differ too.
        assert!(
            differing(&plain, &noisy, 300) > 0,
            "noise must reach the bottom of the frame"
        );
    }

    /// The fixture's own proof (`docs/31` `P1.01` REFACTOR): if a matcher that does nothing but
    /// compare per-row fingerprints cannot recover the script, the fixture is lying and no later
    /// `P1` result means anything. This is deliberately *not* the estimator `P1.05` will build —
    /// it has no downsampling, no confidence and no gate — so it can only be right about the
    /// geometry, which is exactly what is being checked.
    #[test]
    fn the_simplest_estimator_recovers_every_scripted_step() {
        let image = TestImage::synthetic(640, 4_000, 0x5EED);
        // Steps that are small, negative, zero, and exactly one band / one carrier period long.
        let steps = vec![
            StepSpec::move_by(120),
            StepSpec::move_by(120),
            StepSpec::repeat(),
            StepSpec::move_by(-120),
            StepSpec::jump_to(360),
            StepSpec::move_by(1),
            StepSpec::move_by(19),
            StepSpec::move_by(37),
            StepSpec::move_by(40),
            StepSpec::move_by(240),
            StepSpec::move_by(-7),
            StepSpec::move_by(600),
        ];
        let mut script = ScrollScript::new(&image, 900, steps.clone());
        let frames: Vec<TestFrame> = (0..script.len()).map(|k| script.take(k)).collect();

        for k in 1..frames.len() {
            let delta = steps[k - 1].delta;
            let found = simplest_estimator(&frames[k - 1], &frames[k], 700);
            if delta.abs() * 2 <= 900 {
                assert_eq!(
                    found,
                    Some(delta),
                    "frame {k}: the row-fingerprint matcher found {found:?}, the script says {delta}"
                );
            } else {
                // `docs/30` §16.5: past half a viewport the frames barely overlap, the problem is
                // under-determined, and *no* matcher may be required to recover it. The fixture
                // still has to produce such a frame — the `Lost` path exists for it — but the
                // self-proof stops here and says so, instead of quietly dropping the case.
                assert_ne!(
                    found,
                    Some(delta),
                    "a step that leaves less than half the viewport overlapping must not be \
                     recoverable by a matcher that requires that overlap"
                );
            }
        }
    }

    /// FNV-1a over one row. Cheap, deterministic, and fine-grained enough that two rows are
    /// equal only if their bytes are.
    fn row_fingerprints(frame: &TestFrame) -> Vec<u64> {
        let stride = frame.width() as usize * 4;
        frame
            .pixels
            .chunks_exact(stride)
            .map(|row| {
                let mut h = 0xcbf2_9ce4_8422_2325u64;
                for byte in row {
                    h ^= u64::from(*byte);
                    h = h.wrapping_mul(0x0000_0100_0000_01B3);
                }
                h
            })
            .collect()
    }

    /// The simplest possible matcher: count rows that agree between the two frames for each
    /// candidate shift, require at least half the viewport to overlap (`docs/30` §16.5), and take
    /// the largest count with the smallest `|d|` as the tie-break.
    fn simplest_estimator(previous: &TestFrame, current: &TestFrame, window: i32) -> Option<i32> {
        let a = row_fingerprints(previous);
        let b = row_fingerprints(current);
        assert_eq!(a.len(), b.len(), "both frames of a script have one viewport");
        let height = a.len() as i32;

        let mut best: Option<(usize, i32)> = None;
        for d in -window..=window {
            if (height - d.abs()) * 2 < height {
                continue;
            }
            let matches = (0..height)
                .filter(|y| {
                    let j = y + d;
                    (0..height).contains(&j) && b[*y as usize] == a[j as usize]
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
}
