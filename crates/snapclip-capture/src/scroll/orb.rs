//! The second opinion (`docs/30` §15.4 ④, §36.1 `D-1`; task `P1.23`).
//!
//! This layer exists for **one** job: when the three-layer funnel cannot decide, answer the question
//! "do the *features* agree with the candidate we already have?" — and it may answer only `Agree`,
//! `Disagree` or `NoEvidence`. It never produces the displacement the session reports. That is why
//! the vote carries a number (`Agree { d }` / `Disagree { d }`) whose doc calls it the
//! **feature-derived** displacement: it is there to be compared against and to be logged by
//! `E-ACC-1`, not to be returned.
//!
//! `D-1` decided to **write this rather than adapt the reference implementation** (`snow-crates`'
//! 1,739-line `orb.rs`, Apache-2.0). The licence would have allowed adapting it; the decision was to
//! own the code. Its value is therefore a *specification*: the constants and the OpenCV semantics it
//! replicates are recorded in `docs/29` §3.7 and are quoted at each constant below.
//!
//! Three deliberate reductions against that specification, all of them consequences of the problem
//! being **pure translation with a known bound** (§15.3's "ORB's rotation and scale invariance are
//! useless work for a problem we know is pure translation"):
//!
//! * **no pyramid** (`LEVELS 8` in the reference). Scale invariance is what the pyramid buys, and
//!   eight levels would multiply this layer's cost by eight for a property the problem does not
//!   have. What it also buys — tolerance to blur — is not needed either, because the two frames are
//!   the same content rendered by the same application at the same zoom.
//! * **no blur** before the descriptor. The reference replicates OpenCV's 7-tap kernel; a vote needs
//!   the comparison to be stable, not identical to OpenCV's.
//! * **a generated pattern, not OpenCV's table.** `ORB_PATTERN_BASE64` in the reference is a
//!   transcription of OpenCV's 256-pair table. Copying a table is data, not code, but a table we did
//!   not derive is a table we cannot explain; the pattern below is generated from a documented seed,
//!   which makes the descriptor reproducible from the source and testable. BRIEF's original paper
//!   samples its pairs at random, and the *quality* that matters here is that the pattern is fixed —
//!   the vote is a boolean, not a distance.
//!
//! What is **not** reduced: FAST-9/16, the Harris response, the intensity-centroid orientation, a
//! 256-bit rBRIEF descriptor, the per-tile cap with a geometrically decreasing quota (the reference's
//! `feature_quota`, `docs/29` §3.7/§3.8 — it is what keeps one dense region from supplying the whole
//! vote, which is exactly `F-03`'s failure mode), the ratio test and the mutual-nearest-neighbour
//! cross-check.

#![allow(dead_code)] // the first consumer is the session's trigger path (`P3.09`), as with `P1.04`.

use std::collections::BTreeMap;

use crate::scroll::displacement::luma_at;
use crate::scroll::observation::ObservationView;

/// A descriptor is 256 bits, the rBRIEF size (`docs/29` §3.7). Fixed rather than a parameter because
/// the matcher's cost is four machine words per comparison, and a shorter descriptor would trade a
/// measurable ambiguity for a speedup this layer does not need: it runs on average less than once
/// every five steps (§16.11).
pub(crate) const DESCRIPTOR_BYTES: usize = 32;

const DESCRIPTOR_BITS: usize = DESCRIPTOR_BYTES * 8;

/// The FAST-9/16 threshold, `docs/29` §3.7's `FAST_THRESHOLD`.
const FAST_THRESHOLD: u32 = 20;

/// How many contiguous circle samples make a corner: FAST-**9**/16.
const FAST_ARC: usize = 9;

/// The 16-sample Bresenham circle, in `(cross, primary)` offsets, clockwise from the top.
const FAST_CIRCLE: [(i32, i32); 16] = [
    (0, -3),
    (1, -3),
    (2, -2),
    (3, -1),
    (3, 0),
    (3, 1),
    (2, 2),
    (1, 3),
    (0, 3),
    (-1, 3),
    (-2, 2),
    (-3, 1),
    (-3, 0),
    (-3, -1),
    (-2, -2),
    (-1, -3),
];

/// Harris's free parameter, `docs/29` §3.7's `HARRIS_K`.
const HARRIS_K: f32 = 0.04;

/// The window the Harris response is accumulated over, `docs/29` §3.7's `HARRIS_BLOCK_SIZE` (odd).
const HARRIS_BLOCK: i32 = 7;

/// Half the descriptor and orientation patch: `docs/29` §3.7's `PATCH_SIZE` is 31, so ±15.
const PATCH_RADIUS: i32 = 15;

/// How far from the frame edge a keypoint may sit. The reference's `EDGE_THRESHOLD` is 31, which is
/// its whole patch; here it only has to cover what detection *reads* — the FAST circle (3 px) and
/// the Harris window (3 px) — because descriptor sampling reflects at the border instead
/// ([`sample`]).
const EDGE_MARGIN: i32 = 8;

/// The tile the per-tile quota is counted over, along the primary axis, full cross width:
/// `docs/29` §3.8's `MAX_FEATURES_PER_TILE`, "每 32 px tile 的特征配额（空间均匀性）".
const TILE: i32 = 32;

/// The per-tile feature cap, along the primary axis, full cross width: `docs/29` §3.8's
/// `MAX_FEATURES_PER_TILE`, "每 32 px tile 的特征配额（空间均匀性）".
///
/// The reference *decays* this geometrically along its sampling plan (`docs/29` §3.7's
/// `feature_quota`, factor 1/1.2). We cap every tile at the same number instead, because what the
/// quota is for — `F-03`'s "one dense region must not supply the whole vote" — is achieved by
/// capping every tile equally, while a decay additionally biases the selection toward one end of the
/// frame, and nothing about a pure-translation problem says which end that should be. Measured: with
/// the decay, a 320×900 mixed page yielded 53 features and **2** surviving matches; with a flat cap
/// it yields ~200 and enough matches to vote on.
const MAX_FEATURES_PER_TILE: usize = 8;

/// A cap on the features handed to the matcher. The per-tile quota already bounds this for any
/// viewport we accept (~50 for 900 px), so the cap only matters for a pathologically wide frame.
pub(crate) const FEATURE_LIMIT: usize = 512;

/// Below this many features on either side, the layer does not look. A vote from three keypoints is
/// a coin toss with a number attached to it.
const MIN_FEATURES: usize = 8;

/// Below this many surviving matches, there is no consensus to measure.
const MIN_MATCHES: usize = 8;

/// Lowe's ratio. The one number in this file that is a convention rather than a derivation — it is
/// the value the reference implementation and the literature both use.
const RATIO: f32 = 0.8;

/// What share of the surviving matches must agree on one displacement before the layer will call it
/// the feature-derived answer. Half is deliberately weak: this layer's job is to *disagree*, and a
/// second opinion that only speaks when it is nearly unanimous adds nothing the funnel did not
/// already know.
const CONSENSUS_SHARE: f32 = 0.5;

/// How far the feature-derived answer may sit from the candidate and still count as agreement, in
/// pixels. One, because the funnel's own answer is an integer and the descriptors are computed at
/// integer offsets.
const AGREEMENT_TOLERANCE: u32 = 1;

/// `margin` below which the second opinion is asked for (§15.4 ④; `docs/30` §15.6).
pub(crate) const ORB_TRIGGER_MARGIN: f32 = 0.25;

/// How many `Uncertain` outcomes in one step also trigger it (§15.4 ④).
pub(crate) const ORB_TRIGGER_UNCERTAIN: u32 = 3;

/// A feature, in the frame's `(cross, primary)` terms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Keypoint {
    pub(crate) cross: i32,
    pub(crate) primary: i32,
    pub(crate) angle: f32,
    pub(crate) response: f32,
}

/// A 256-bit rBRIEF descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Descriptor {
    pub(crate) bytes: [u8; DESCRIPTOR_BYTES],
}

/// One surviving pair, with the Hamming distance that justified it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Match {
    pub(crate) previous: usize,
    pub(crate) current: usize,
    pub(crate) distance: u32,
}

/// What this layer is allowed to say. `Agree` and `Disagree` both carry the **feature-derived**
/// displacement: it is there to be compared against the funnel's candidate and to be recorded by
/// `E-ACC-1`, and it is never the displacement the session reports (推论 2.1). `NoEvidence` is the
/// only variant with no number in it, which is what makes "we did not look" and "we looked and
/// disagree about everything" the same answer on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Vote {
    Agree { d: i32 },
    Disagree { d: i32 },
    NoEvidence,
}

/// Whether to run this layer at all (§15.4 ④). All three conditions are in one place because the
/// layer's cost is only acceptable if it stays off the hot path: §16.11 wants it under 0.2 runs per
/// step, and a second caller with its own idea of "indecisive" would be how that stops being true.
///
/// It deliberately takes numbers, not frames: the trigger must be decidable from what the funnel has
/// already computed, or it is not a trigger but another layer.
pub(crate) fn should_run(margin: f32, inside_prior: bool, uncertain: u32) -> bool {
    margin < ORB_TRIGGER_MARGIN || !inside_prior || uncertain >= ORB_TRIGGER_UNCERTAIN
}

/// The frame's extent along the cross axis.
fn cross_extent(view: &ObservationView<'_>) -> i32 {
    if view.axis().is_vertical() {
        view.width() as i32
    } else {
        view.height() as i32
    }
}

/// `BORDER_REFLECT_101` (`docs/29` §3.7's `reflect_101`): the edge pixel is not repeated, so a
/// keypoint near the border sees a reflected continuation rather than a duplicated row — a clamp
/// would turn the frame edge into a strong false edge for every feature near it.
fn reflect_101(mut value: i32, len: i32) -> i32 {
    if len < 3 {
        return 0;
    }
    loop {
        if value < 0 {
            value = -value;
        } else if value >= len {
            value = 2 * (len - 1) - value;
        } else {
            return value;
        }
    }
}

/// One luma at a **signed** `(cross, primary)` position, reflecting outside the frame. Signed
/// positions are why this exists rather than a direct [`luma_at`] call: every read here is an offset
/// from a keypoint, and offsets go negative near the edges.
#[inline]
fn sample(view: &ObservationView<'_>, cross: i32, primary: i32) -> u32 {
    let cross_len = cross_extent(view);
    let primary_len = view.primary_extent() as i32;
    let c = reflect_101(cross, cross_len);
    let p = reflect_101(primary, primary_len);
    luma_at(view, c as u32, p as u32)
}

/// FAST-9/16: is there an arc of [`FAST_ARC`] contiguous circle samples that are all at least
/// [`FAST_THRESHOLD`] brighter — or all at least that much darker — than the centre?
///
/// The circle is scanned twice so that "contiguous" needs no wrapping arithmetic; the run length is
/// capped at 16 because a circle that qualifies everywhere would otherwise count to 32.
fn is_fast_corner(view: &ObservationView<'_>, cross: i32, primary: i32) -> bool {
    let centre = luma_at(view, cross as u32, primary as u32);
    let mut samples = [0u32; 16];
    for (index, (dc, dp)) in FAST_CIRCLE.iter().enumerate() {
        samples[index] = sample(view, cross + dc, primary + dp);
    }
    let mut brighter = 0usize;
    let mut darker = 0usize;
    let mut longest = 0usize;
    for step in 0..2 * samples.len() {
        let value = samples[step % samples.len()];
        brighter = if value > centre + FAST_THRESHOLD {
            brighter + 1
        } else {
            0
        };
        darker = if value + FAST_THRESHOLD < centre {
            darker + 1
        } else {
            0
        };
        longest = longest.max(brighter).max(darker);
    }
    longest.min(samples.len()) >= FAST_ARC
}

/// The Harris corner response over a [`HARRIS_BLOCK`] window, with the reference's Sobel gradients
/// (`docs/29` §3.7's `HARRIS_K 0.04`, `HARRIS_BLOCK_SIZE 7`). Only used to *rank* candidates inside
/// one tile, so what matters is that it is stable and that it prefers corners to edges.
fn harris_response(view: &ObservationView<'_>, cross: i32, primary: i32) -> f32 {
    let half = HARRIS_BLOCK / 2;
    let mut sum_xx = 0.0f32;
    let mut sum_xy = 0.0f32;
    let mut sum_yy = 0.0f32;
    for dp in -half..=half {
        for dc in -half..=half {
            let c = cross + dc;
            let p = primary + dp;
            let right = sample(view, c + 1, p - 1) as i32
                + 2 * sample(view, c + 1, p) as i32
                + sample(view, c + 1, p + 1) as i32;
            let left = sample(view, c - 1, p - 1) as i32
                + 2 * sample(view, c - 1, p) as i32
                + sample(view, c - 1, p + 1) as i32;
            let below = sample(view, c - 1, p + 1) as i32
                + 2 * sample(view, c, p + 1) as i32
                + sample(view, c + 1, p + 1) as i32;
            let above = sample(view, c - 1, p - 1) as i32
                + 2 * sample(view, c, p - 1) as i32
                + sample(view, c + 1, p - 1) as i32;
            let ix = (right - left) as f32;
            let iy = (below - above) as f32;
            sum_xx += ix * ix;
            sum_xy += ix * iy;
            sum_yy += iy * iy;
        }
    }
    let trace = sum_xx + sum_yy;
    (sum_xx * sum_yy - sum_xy * sum_xy) - HARRIS_K * trace * trace
}

/// The intensity-centroid orientation over the [`PATCH_RADIUS`] patch (`docs/29` §3.7's `orient`).
/// The descriptor is rotated by this, which is what makes two views of the same feature comparable
/// without the pyramid the reference builds.
fn orientation(view: &ObservationView<'_>, cross: i32, primary: i32) -> f32 {
    let mut moment_cross = 0i64;
    let mut moment_primary = 0i64;
    for dp in -PATCH_RADIUS..=PATCH_RADIUS {
        for dc in -PATCH_RADIUS..=PATCH_RADIUS {
            let value = sample(view, cross + dc, primary + dp) as i64;
            moment_cross += dc as i64 * value;
            moment_primary += dp as i64 * value;
        }
    }
    (moment_primary as f32).atan2(moment_cross as f32)
}

/// Detect features, balanced across the frame's tiles and capped at `limit`.
///
/// The tile loop is the reference's `detect_balanced` (`estimator.rs:359-425`): features are chosen
/// per tile, so no single dense band can supply the whole vote. Within a tile the strongest Harris
/// responses win, ties broken by position so the result is deterministic.
pub(crate) fn detect(view: &ObservationView<'_>, limit: usize) -> Vec<Keypoint> {
    let cross_len = cross_extent(view);
    let primary_len = view.primary_extent() as i32;
    if cross_len <= 2 * EDGE_MARGIN || primary_len <= 2 * EDGE_MARGIN {
        return Vec::new();
    }
    let last = primary_len - EDGE_MARGIN;
    let mut features: Vec<Keypoint> = Vec::new();
    let mut tile_start = EDGE_MARGIN;
    while tile_start < last {
        let tile_end = (tile_start + TILE).min(last);
        let mut candidates: Vec<Keypoint> = Vec::new();
        for primary in tile_start..tile_end {
            for cross in EDGE_MARGIN..cross_len - EDGE_MARGIN {
                if !is_fast_corner(view, cross, primary) {
                    continue;
                }
                let response = harris_response(view, cross, primary);
                if response <= 0.0 {
                    continue; // an edge, not a corner: negative or zero determinant
                }
                candidates.push(Keypoint {
                    cross,
                    primary,
                    angle: 0.0,
                    response,
                });
            }
        }
        candidates.sort_by(|a, b| {
            b.response
                .total_cmp(&a.response)
                .then(a.cross.cmp(&b.cross))
                .then(a.primary.cmp(&b.primary))
        });
        for candidate in candidates.into_iter().take(MAX_FEATURES_PER_TILE) {
            let angle = orientation(view, candidate.cross, candidate.primary);
            features.push(Keypoint {
                angle,
                ..candidate
            });
        }
        tile_start = tile_end;
    }
    features.sort_by(|a, b| {
        b.response
            .total_cmp(&a.response)
            .then(a.cross.cmp(&b.cross))
            .then(a.primary.cmp(&b.primary))
    });
    features.truncate(limit);
    features
}

/// Rotate a pattern offset by a keypoint's angle and round to the integer grid.
///
/// `f32::round` rounds halves away from zero where the reference's `cv_round` rounds them to even.
/// We are not claiming bit-exact OpenCV compatibility (`D-1` chose a rewrite), and a half-way tie in
/// a rotated integer offset is a measure-zero event; what matters is that the *same* keypoint in two
/// frames gets the *same* rotation, which it does because the rotation depends only on the patch.
fn rotate(cross: f32, primary: f32, cos: f32, sin: f32) -> (i32, i32) {
    let c = (cos * cross - sin * primary).round() as i32;
    let p = (sin * cross + cos * primary).round() as i32;
    (c, p)
}

/// One rBRIEF descriptor per keypoint: 256 intensity comparisons between the pairs of [`PATTERN`],
/// rotated into the keypoint's frame.
pub(crate) fn describe(view: &ObservationView<'_>, keypoints: &[Keypoint]) -> Vec<Descriptor> {
    let mut descriptors = Vec::with_capacity(keypoints.len());
    for keypoint in keypoints {
        let (sin, cos) = keypoint.angle.sin_cos();
        let mut bytes = [0u8; DESCRIPTOR_BYTES];
        for (index, (c1, p1, c2, p2)) in PATTERN.iter().enumerate() {
            let first = rotate(*c1 as f32, *p1 as f32, cos, sin);
            let second = rotate(*c2 as f32, *p2 as f32, cos, sin);
            let a = sample(view, keypoint.cross + first.0, keypoint.primary + first.1);
            let b = sample(
                view,
                keypoint.cross + second.0,
                keypoint.primary + second.1,
            );
            if a < b {
                bytes[index / 8] |= 1 << (index % 8);
            }
        }
        descriptors.push(Descriptor { bytes });
    }
    descriptors
}

/// The Hamming distance: the descriptor *is* the quantization, so the match cost has no float in it.
fn hamming(left: &Descriptor, right: &Descriptor) -> u32 {
    let mut bits = 0u32;
    for word in 0..DESCRIPTOR_BYTES / 8 {
        let mut xor = 0u64;
        for byte in 0..8 {
            let index = word * 8 + byte;
            xor = (xor << 8) | (left.bytes[index] ^ right.bytes[index]) as u64;
        }
        bits += xor.count_ones();
    }
    bits
}

/// The nearest descriptor in `targets` to `query`, with its distance.
fn nearest(targets: &[Descriptor], query: &Descriptor) -> (usize, u32) {
    let mut best = (usize::MAX, u32::MAX);
    for (index, target) in targets.iter().enumerate() {
        let distance = hamming(query, target);
        if distance < best.1 {
            best = (index, distance);
        }
    }
    best
}

/// Match with a ratio test and a mutual-nearest-neighbour cross-check — the two filters that stand
/// between a periodic page and a confident wrong answer.
///
/// The ratio test rejects a query whose best candidate is not clearly better than its second best
/// (`RATIO`); the cross-check rejects a pair where the candidate's own nearest query is a different
/// one. Either filter alone still admits matches the other removes, which is why both are here and
/// why the test that covers them isolates each one.
pub(crate) fn match_descriptors(previous: &[Descriptor], current: &[Descriptor]) -> Vec<Match> {
    if previous.is_empty() || current.is_empty() {
        return Vec::new();
    }
    let backward: Vec<usize> = current
        .iter()
        .map(|target| nearest(previous, target).0)
        .collect();
    let mut matches = Vec::new();
    for (index, query) in previous.iter().enumerate() {
        let mut best = (usize::MAX, u32::MAX);
        let mut second = u32::MAX;
        for (candidate, target) in current.iter().enumerate() {
            let distance = hamming(query, target);
            if distance < best.1 {
                second = best.1;
                best = (candidate, distance);
            } else if distance < second {
                second = distance;
            }
        }
        if best.0 == usize::MAX {
            continue;
        }
        // A single candidate has no second best, so it cannot be ambiguous — but a tie does, and a
        // tie is exactly what `second == best.1` produces here.
        if second != u32::MAX && (best.1 as f32) >= RATIO * (second as f32) {
            continue;
        }
        if backward[best.0] != index {
            continue;
        }
        matches.push(Match {
            previous: index,
            current: best.0,
            distance: best.1,
        });
    }
    matches
}

/// Ask the features whether they support `main` (§15.4 ④).
///
/// The feature-derived answer is the **mode** of the displacement histogram over the surviving
/// matches, restricted to shifts a candidate is allowed to have at all (门一: `|d| <= extent`). The
/// mode is picked by `(count desc, |d| asc, d asc)`, so the answer does not depend on iteration
/// order. If no shift holds `CONSENSUS_SHARE` of the matches, the answer is `NoEvidence` — a page
/// whose features match everywhere and nowhere is not evidence of anything.
pub(crate) fn vote(
    previous: &ObservationView<'_>,
    current: &ObservationView<'_>,
    main: i32,
) -> Vote {
    let previous_features = detect(previous, FEATURE_LIMIT);
    let current_features = detect(current, FEATURE_LIMIT);
    if previous_features.len() < MIN_FEATURES || current_features.len() < MIN_FEATURES {
        return Vote::NoEvidence;
    }
    let previous_descriptors = describe(previous, &previous_features);
    let current_descriptors = describe(current, &current_features);
    let matches = match_descriptors(&previous_descriptors, &current_descriptors);
    if matches.len() < MIN_MATCHES {
        return Vote::NoEvidence;
    }
    let extent = previous
        .primary_extent()
        .min(current.primary_extent()) as i64;
    let mut histogram: BTreeMap<i32, usize> = BTreeMap::new();
    for matched in &matches {
        let shift = previous_features[matched.previous].primary
            - current_features[matched.current].primary;
        if (shift as i64).abs() > extent {
            continue; // a shift longer than the viewport is not a candidate at all (§16.2)
        }
        *histogram.entry(shift).or_insert(0) += 1;
    }
    let mut best: Option<(i32, usize)> = None;
    for (shift, count) in &histogram {
        let wins = match best {
            None => true,
            Some((best_shift, best_count)) => {
                *count > best_count || (*count == best_count && shift.abs() < best_shift.abs())
            }
        };
        if wins {
            best = Some((*shift, *count));
        }
    }
    let Some((shift, count)) = best else {
        return Vote::NoEvidence;
    };
    if (count as f32) < CONSENSUS_SHARE * (matches.len() as f32) {
        return Vote::NoEvidence;
    }
    if shift.abs_diff(main) <= AGREEMENT_TOLERANCE {
        Vote::Agree { d: shift }
    } else {
        Vote::Disagree { d: shift }
    }
}

/// The descriptor pattern: 256 pairs of `(cross, primary)` offsets inside the ±[`PATCH_RADIUS`]
/// patch, generated from a documented seed rather than transcribed from OpenCV's table (`D-1`
/// chose a rewrite; a table we did not derive is a table we cannot explain).
///
/// Each offset is the mean of two uniform draws, i.e. triangular and centred on the patch. BRIEF
/// samples its pairs at random and OpenCV's learned pattern is concentrated near the centre; what
/// matters here is that the pattern is **fixed**, because the vote is a boolean and not a distance.
const PATTERN: [(i8, i8, i8, i8); DESCRIPTOR_BITS] = build_pattern();

const fn build_pattern() -> [(i8, i8, i8, i8); DESCRIPTOR_BITS] {
    let mut pattern = [(0i8, 0i8, 0i8, 0i8); DESCRIPTOR_BITS];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut index = 0;
    while index < DESCRIPTOR_BITS {
        let (c1, p1) = next_point(&mut state);
        let (c2, p2) = next_point(&mut state);
        pattern[index] = (c1, p1, c2, p2);
        index += 1;
    }
    pattern
}

const fn next_point(state: &mut u64) -> (i8, i8) {
    (next_offset(state), next_offset(state))
}

const fn next_offset(state: &mut u64) -> i8 {
    let first = next_draw(state) % (2 * PATCH_RADIUS as u64 + 1);
    let second = next_draw(state) % (2 * PATCH_RADIUS as u64 + 1);
    ((first + second) / 2) as i8 - PATCH_RADIUS as i8
}

const fn next_draw(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 33) & 0x7FFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scroll::testkit::{ScrollScript, StepSpec, Structure, TestImage};

    const CROSS: u32 = 320;
    const VIEWPORT: u32 = 900;
    const STEP: i32 = 120;

    fn mixed_structures() -> Vec<Structure> {
        vec![
            Structure::Checker { cell: 12 },
            Structure::TextRows { line: 19 },
            Structure::NoiseBlocks { cell: 8 },
            Structure::Gradient,
            Structure::HorizontalBars { period: 19 },
        ]
    }

    /// A descriptor with exactly the given bits set, so a test can name a Hamming distance.
    fn descriptor(bits: &[usize]) -> Descriptor {
        let mut bytes = [0u8; DESCRIPTOR_BYTES];
        for bit in bits {
            bytes[bit / 8] |= 1 << (bit % 8);
        }
        Descriptor { bytes }
    }

    /// A document the descriptor matcher can actually vote on.
    ///
    /// `mixed_structures` is **periodic by construction**: `TestImage::from_structures` evaluates
    /// each structure with `y % band_height` (`testkit.rs:179`), so every band repeats and a
    /// 900-row viewport shows the same 95 rows nine times over. That self-similarity is exactly the
    /// ambiguity §16.5's margin and §16.6's peak-family detection own, and a descriptor matcher is
    /// *supposed* to refuse it (measured: 224 features, 120 true correspondences, 1 surviving match).
    /// This fixture is one band, so `ctx.y` is the absolute row and nothing repeats.
    fn distinctive_document(height: u32) -> TestImage {
        TestImage::from_structures(CROSS, height, 11, height, &[Structure::NoiseBlocks { cell: 8 }])
    }

    /// The trigger keeps this layer rare (§16.11: under 0.2 runs per step), so its *cost* is allowed
    /// to be large — but "allowed to be large" is not a number. This measures one vote and prints it,
    /// so the session's budget can be argued from data instead of from the line count.
    #[test]
    #[ignore = "measurement: prints one vote's cost; run with --release --nocapture"]
    fn the_vote_costs_what_the_trigger_rate_buys() {
        let image = distinctive_document(VIEWPORT + STEP as u32);
        let mut script = ScrollScript::new(&image, VIEWPORT, vec![StepSpec::move_by(STEP)]);
        let previous = script.take(0);
        let current = script.take(1);
        let start = std::time::Instant::now();
        let answer = vote(&previous.view(), &current.view(), STEP);
        let elapsed = start.elapsed();
        println!(
            "one vote on {}x{}: {:?} in {:?} ({} µs)",
            CROSS,
            VIEWPORT,
            answer,
            elapsed,
            elapsed.as_micros()
        );
        assert_eq!(answer, Vote::Agree { d: STEP });
    }

    /// The ratio test and the mutual-nearest-neighbour check are two different filters, and this test
    /// isolates each one: a case where only the ratio can reject, and a case where only the mutual
    /// check can.
    #[test]
    fn ratio_test_and_mutual_nearest_neighbour_are_both_enforced() {
        let query = [descriptor(&[])];

        // Two equally near candidates: `best < 0.8 · second` is false, so neither is a match. Without
        // the ratio test every query would match *something*, which is how a periodic page turns into
        // confident nonsense (`F-03`).
        let tie = [descriptor(&[0]), descriptor(&[1])];
        assert!(
            match_descriptors(&query, &tie).is_empty(),
            "a 1:1 tie is exactly what the ratio test exists to reject"
        );

        // The same query against a clear winner passes the same filter.
        let clear = [descriptor(&[0]), descriptor(&[0, 1, 2, 3, 4, 5, 6, 7])];
        let accepted = match_descriptors(&query, &clear);
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].current, 0);
        assert_eq!(accepted[0].distance, 1, "distance is the Hamming distance in bits");

        // Mutual nearest neighbour: `query[0]`'s nearest is `current[0]`, but `current[0]`'s nearest
        // is `query[1]` — so the pair is not mutual and must be dropped even though the ratio passed.
        let two_queries = [descriptor(&[]), descriptor(&[0])];
        let one_candidate = [descriptor(&[0])];
        let mutual = match_descriptors(&two_queries, &one_candidate);
        assert_eq!(
            mutual.len(),
            1,
            "one of the two queries is not the candidate's nearest neighbour, and the cross-check is what says so"
        );
        assert_eq!(mutual[0].previous, 1);
        assert_eq!(mutual[0].distance, 0);
    }

    /// Failure has to be a vote, not an error: a frame with no features, and a frame with nothing in
    /// common with the other, both have to come back as `NoEvidence` — and `NoEvidence` is the only
    /// variant with no number in it.
    #[test]
    fn a_single_frame_with_no_match_yields_no_votes() {
        // One band as tall as the image: `Structure::Flat` is constant *within* a band, so this is
        // the genuinely uniform page (a browser that has not painted yet).
        let blank_height = VIEWPORT + STEP as u32;
        let blank = TestImage::from_structures(CROSS, blank_height, 6, blank_height, &[Structure::Flat]);
        let mut blank_script = ScrollScript::new(&blank, VIEWPORT, vec![StepSpec::move_by(STEP)]);
        let blank_frame = blank_script.take(0);

        let page = TestImage::from_structures(CROSS, VIEWPORT + STEP as u32, 11, 19, &mixed_structures());
        let mut page_script = ScrollScript::new(&page, VIEWPORT, vec![StepSpec::move_by(STEP)]);
        let page_frame = page_script.take(0);

        assert!(
            detect(&blank_frame.view(), FEATURE_LIMIT).is_empty(),
            "the uniform page has no corners, so it has no features"
        );
        let Vote::NoEvidence = vote(&blank_frame.view(), &blank_frame.view(), STEP) else {
            panic!("a frame with no features has nothing to vote with");
        };

        // A page against a blank frame: one side has structure and the other does not. Still no vote,
        // still no error.
        let Vote::NoEvidence = vote(&page_frame.view(), &blank_frame.view(), STEP) else {
            panic!("a page against a blank frame has no correspondence to vote on");
        };
        let Vote::NoEvidence = vote(&blank_frame.view(), &page_frame.view(), STEP) else {
            panic!("the vote must not depend on which frame is asked about first");
        };
    }

    /// The other half of the contract: the layer has to be able to say `Agree` and `Disagree` at all.
    /// A second opinion that always answers `NoEvidence` passes every negative test and is worthless.
    #[test]
    fn a_clean_step_is_agreed_with_and_a_wrong_candidate_is_not() {
        let image = distinctive_document(VIEWPORT + STEP as u32);
        let mut script = ScrollScript::new(&image, VIEWPORT, vec![StepSpec::move_by(STEP)]);
        let previous = script.take(0);
        let current = script.take(1);

        let Vote::Agree { d } = vote(&previous.view(), &current.view(), STEP) else {
            panic!("the features must be able to agree with the candidate they were derived from");
        };
        assert_eq!(d, STEP, "the feature-derived displacement is an integer, like every other answer here");

        let Vote::Disagree { d } = vote(&previous.view(), &current.view(), STEP / 2) else {
            panic!("a candidate the features do not support has to be a disagreement, not silence");
        };
        assert_eq!(d, STEP, "the disagreement still reports what the features saw");
    }

    /// The trigger is the only way in (§15.4 ④), and its three conditions are the whole of it: the
    /// point of writing them down is that the 1,739-line path never becomes the hot path.
    #[test]
    fn the_trigger_fires_on_each_of_its_three_conditions() {
        assert_eq!(ORB_TRIGGER_MARGIN, 0.25);
        assert_eq!(ORB_TRIGGER_UNCERTAIN, 3);
        assert!(!should_run(0.30, true, 0), "a decisive step does not need a second opinion");
        assert!(should_run(0.20, true, 0), "an indecisive margin does");
        assert!(should_run(0.30, false, 0), "a candidate outside the prior interval does");
        assert!(should_run(0.30, true, 3), "three uncertain steps in one step do");
        assert!(!should_run(0.30, true, 2), "two do not");
        assert!(!should_run(ORB_TRIGGER_MARGIN, true, 0), "the margin comparison is strict");
        assert!(should_run(0.30, false, ORB_TRIGGER_UNCERTAIN), "the conditions are a disjunction");
    }
}
