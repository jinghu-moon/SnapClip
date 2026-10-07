//! The ring's paint decision, as a pure function of what it sits on (docs/21 §5.24, A4).
//!
//! **Why this exists now.** A4 ("sample the screenshot and let the rings adapt") needs one input —
//! the composited colour under a ring — and the tables that said what that colour is were written
//! *before* A2. A2 punches the mask hole at the box being captured and cuts the hover wash around it,
//! so the **inner rings now sit on the raw frozen frame** instead of on "mask + hover wash + preview
//! wash". That is not a detail: the old stack compressed every page into L 0.047–0.287, the new one
//! spans L 0.005–0.956, and the shipped ring tone at the faintest ring's alpha goes from 1.71–4.38:1
//! to **1.11–6.47:1** across that range. The pre-A2 tables are void; this module is the recomputation,
//! and its tests pin the new numbers (docs/21 §5.24.11).
//!
//! **What it does not do.** It samples nothing. The sampler (§5.24.6) reads pixels and hands them to
//! [`ring_style_for`]; keeping the *decision* separate is what lets the part that has to be right be
//! tested without a GPU.
//!
//! **The threshold.** `3:1` here is the *model's* bar. Measured against rendered pixels the model runs
//! 10–30 % high (a 1.5 px anti-aliased stroke never lands a full pixel on its own core), so 3:1 in the
//! model is the ≈2.5:1 the ring gate already uses (§5.24.4).

/// An sRGB colour, one byte per channel — the space a readback arrives in and the space the overlay
/// blends in, so the arithmetic below matches what the compositor will actually do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    fn channels(self) -> [f32; 3] {
        [self.r as f32, self.g as f32, self.b as f32]
    }

    /// The same channels as `0.0..=1.0`, which is the space the paint layer's colours live in.
    pub fn channels_f32(self) -> (f32, f32, f32) {
        (
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
        )
    }

    fn from_channels(channels: [f32; 3]) -> Self {
        let channel = |value: f32| value.clamp(0.0, 255.0).round() as u8;
        Self::new(
            channel(channels[0]),
            channel(channels[1]),
            channel(channels[2]),
        )
    }
}

/// Linear interpolation between two palette colours; `t = 0` gives `from`.
///
/// Shared with the paint layer, which uses it for the capture box's stroke (brand blue → capture
/// green) — the same interpolation the decision would reason about.
pub fn mix(from: Rgb, to: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let from = from.channels();
    let to = to.channels();
    Rgb::from_channels([
        from[0] + (to[0] - from[0]) * t,
        from[1] + (to[1] - from[1]) * t,
        from[2] + (to[2] - from[2]) * t,
    ])
}

// ── The palette and the layer alphas ────────────────────────────────────────────────────────────
//
// These live here, not in the paint layer, because two things must never drift: what the overlay
// *paints* and what this decision *believes* it painted. The paint layer imports them (docs/21
// §5.24.6 called this the one real drift risk of A4).

/// The neutral mask: pure black, 45 % (docs/21 §5.22 measured it).
pub const MASK_RGB: Rgb = Rgb::new(0, 0, 0);
pub const MASK_ALPHA: f32 = 0.45;
/// The brand trace on top of the mask. 10 % is the measured ceiling: at 30 % the screen shifts towards
/// the rings' hue and a blue ring drops from 3.1:1 to 1.2–1.8:1.
pub const MASK_TINT_ALPHA: f32 = 0.10;
/// The brand blue: chrome, the hovered window's outline, and the capture box at rest.
pub const ACCENT_RGB: Rgb = Rgb::new(31, 117, 219);
/// The capture green: the box that would be taken, while the walk is live.
pub const CAPTURE_RGB: Rgb = Rgb::new(27, 177, 95);
/// The chain's ring tone (docs/21 §5.22).
pub const CHAIN_RING_RGB: Rgb = Rgb::new(185, 217, 255);
/// The dark underlay every ring is painted with today — the number A4 would replace per ring.
pub const CHAIN_RING_SHADOW: f32 = 0.80;
/// The hover wash: lifts the hovered window out of the mask.
pub const HOVER_WASH_ALPHA: f32 = 0.10;
/// The capture wash at **full walking**, before [`PREVIEW_WASH_WALK_SCALE`].
pub const PREVIEW_WASH_ALPHA: f32 = 0.18;
/// Walks get half the wash.
pub const PREVIEW_WASH_WALK_SCALE: f32 = 0.5;

/// `over` painted on top of `under` at `alpha`, in sRGB.
fn mix_over(over: Rgb, under: Rgb, alpha: f32) -> Rgb {
    let alpha = alpha.clamp(0.0, 1.0);
    let over = over.channels();
    let under = under.channels();
    Rgb::from_channels([
        over[0] * alpha + under[0] * (1.0 - alpha),
        over[1] * alpha + under[1] * (1.0 - alpha),
        over[2] * alpha + under[2] * (1.0 - alpha),
    ])
}

/// The pixel under a ring **outside** the hole: masked, then washed by the hover veil (docs/21
/// §5.24.4). Unchanged by A2 — outer rings are outside the hole and the hover wash is only cut
/// *around* it.
pub fn outer_ring_background(raw: Rgb) -> Rgb {
    let masked = mix_over(MASK_RGB, raw, MASK_ALPHA);
    let tinted = mix_over(ACCENT_RGB, masked, MASK_TINT_ALPHA);
    mix_over(Rgb::new(255, 255, 255), tinted, HOVER_WASH_ALPHA)
}

/// The pixel under a ring **inside** the hole: the frozen frame itself, plus the capture wash while
/// the walk is live (docs/21 §5.24.11 — this is what A2 changed, and it is why the tables had to be
/// recomputed).
///
/// `walking` is the walk envelope, `0.0..=1.0`; the wash is `18 % × 0.5 × walking`.
pub fn inner_ring_background(raw: Rgb, walking: f32) -> Rgb {
    mix_over(
        CAPTURE_RGB,
        raw,
        PREVIEW_WASH_ALPHA * PREVIEW_WASH_WALK_SCALE * walking.clamp(0.0, 1.0),
    )
}

// ── Legibility ──────────────────────────────────────────────────────────────────────────────────

/// WCAG relative luminance of an sRGB colour.
pub fn relative_luminance(color: Rgb) -> f32 {
    let channel = |value: u8| {
        let value = value as f32 / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

/// WCAG contrast ratio between two colours.
pub fn contrast_ratio(a: Rgb, b: Rgb) -> f32 {
    let (high, low) = {
        let (a, b) = (relative_luminance(a), relative_luminance(b));
        if a > b { (a, b) } else { (b, a) }
    };
    (high + 0.05) / (low + 0.05)
}

/// One ring's paint decision: which tone, and how strong a dark underlay (docs/21 §5.24, A4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingStyle {
    pub tone: Rgb,
    /// The black underlay's alpha; `0.0` means "paint no underlay at all".
    pub halo: f32,
}

/// The halos the decision may pick from, smallest first.
///
/// Deliberately coarse: each step is a visible black band over the user's content, so the point is to
/// find the *smallest* one that works rather than to interpolate.
pub const HALO_LADDER: [f32; 5] = [0.0, 0.2, 0.4, 0.6, 0.8];

/// The bar the pair has to clear (see the module docs for why 3:1 and not 2.5:1).
pub const PAIR_THRESHOLD: f32 = 3.0;

/// What the product paints today: one tone, one halo, everywhere.
pub const SHIPPED_TONES: [Rgb; 1] = [CHAIN_RING_RGB];

/// The prototype's ladder (v3 A4): ring blue, one step lighter, the brand blue, the lightest.
///
/// Under the mask this ladder bought nothing — every page landed in one narrow luminance band where
/// no tone could cross 3:1 that the shipped one could not. Inside the hole it buys something, but
/// **only for the strong rings**: at the faintest ring's alpha (`0.68`) a narrower tone is diluted by
/// the page it is blended with, and even the brand blue only reaches 2.2:1 on raw light content — the
/// underlay is still the lever there. At full alpha the same tone clears 3.6:1 bare (see the tests),
/// so the ladder is a knob for the rings next to the selection, not for the whole chain.
pub const PROTOTYPE_TONES: [Rgb; 4] = [
    Rgb::new(74, 155, 255),
    Rgb::new(109, 176, 255),
    Rgb::new(31, 117, 219),
    Rgb::new(140, 196, 255),
];

/// The legibility of `tone` at `ring_alpha` over `page` with a `halo` underlay.
///
/// This is the **pair** rule the rendered-pixel gate uses: the eye takes whichever side of the stroke
/// is stronger, so a light core on a dark underlay and a dark band on a light page both count.
pub fn pair_contrast(page: Rgb, tone: Rgb, halo: f32, ring_alpha: f32) -> f32 {
    let halo = halo.clamp(0.0, 1.0);
    let band = mix_over(MASK_RGB, page, halo);
    let core = mix_over(tone, band, ring_alpha.clamp(0.0, 1.0));
    contrast_ratio(core, band).max(contrast_ratio(band, page))
}

/// The smallest `(halo, tone)` whose pair clears [`PAIR_THRESHOLD`], or the best the ladder can do.
///
/// Halos are searched outermost-first and tones innermost-first, so "no underlay" always wins over
/// "a dark band over the user's content" when a tone alone can carry the ring, and among tones the
/// caller's order is its preference (the shipped tone first unless told otherwise).
pub fn ring_style_for(page: Rgb, ring_alpha: f32, tones: &[Rgb]) -> RingStyle {
    let tones = if tones.is_empty() { &SHIPPED_TONES } else { tones };
    for halo in HALO_LADDER {
        for tone in tones {
            if pair_contrast(page, *tone, halo, ring_alpha) >= PAIR_THRESHOLD {
                return RingStyle { tone: *tone, halo };
            }
        }
    }
    // Nothing in the ladder reaches the bar: take the strongest halo and the tone that does best
    // there. A ring that cannot be made legible still has to be *as* legible as it can be.
    let halo = *HALO_LADDER.last().expect("the ladder is not empty");
    let tone = *tones
        .iter()
        .max_by(|a, b| {
            pair_contrast(page, **a, halo, ring_alpha)
                .total_cmp(&pair_contrast(page, **b, halo, ring_alpha))
        })
        .expect("tones is not empty");
    RingStyle { tone, halo }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The faintest ring the plan can produce (the ramp's floor, docs/21 §5.22).
    const FAINTEST: f32 = 0.68;

    fn grey(value: u8) -> Rgb {
        Rgb::new(value, value, value)
    }

    /// The page colours the tables are read on: white, light, mid, grey, dark, near-black.
    fn pages() -> [(Rgb, &'static str); 6] {
        [
            (grey(250), "white 250"),
            (grey(230), "light 230"),
            (grey(128), "mid 128"),
            (grey(90), "grey 90"),
            (grey(30), "dark 30"),
            (Rgb::new(16, 16, 18), "near-black 16"),
        ]
    }

    /// **A2 moved the inner rings onto the raw frame.** This is the recomputation
    /// (docs/21 §5.24.11): the hole keeps its own pixels, the hover wash is cut around it, and the
    /// only thing on top is the capture wash while the walk is live.
    #[test]
    fn the_inner_rings_sit_on_the_raw_frame_not_on_the_masked_page() {
        // At rest the wash is zero, so the background *is* the frame — byte for byte.
        for (page, name) in pages() {
            assert_eq!(inner_ring_background(page, 0.0), page, "{name} at rest");
        }
        // …and while walking it is the frame under the capture wash, not under the mask.
        let washed = inner_ring_background(grey(250), 1.0);
        assert!(
            washed.g > washed.r,
            "the walking wash is green and light: {washed:?}"
        );
        assert!(
            relative_luminance(washed) > 0.5,
            "a white page stays light inside the hole: {}",
            relative_luminance(washed)
        );
        // The outer rings are unchanged: they are outside the hole, so the mask and the hover wash
        // are still on them. (The pre-A2 table said `(140,147,157)`; rounding each blend instead of
        // the printed value moves one channel by one.)
        let outer = outer_ring_background(grey(250));
        let pre_a2_outer = Rgb::new(140, 147, 157);
        for (observed, expected) in [(outer.r, pre_a2_outer.r), (outer.g, pre_a2_outer.g), (outer.b, pre_a2_outer.b)] {
            assert!(
                observed.abs_diff(expected) <= 1,
                "the outer ring's background is still the masked one: {outer:?}"
            );
        }
    }

    /// The pre-A2 inner table, kept only here so the test can show what it would cost to still use
    /// it: the numbers A4 was first designed against.
    fn old_inner_background(raw: Rgb, walking: f32) -> Rgb {
        let masked = mix_over(MASK_RGB, raw, MASK_ALPHA);
        let tinted = mix_over(ACCENT_RGB, masked, MASK_TINT_ALPHA);
        let hovered = mix_over(Rgb::new(255, 255, 255), tinted, HOVER_WASH_ALPHA);
        inner_ring_background(hovered, walking)
    }

    /// The whole point of recomputing: the old mask-based numbers are not just slightly off, they
    /// pick the wrong underlay — and using them on the new background leaves the ring unreadable.
    #[test]
    fn the_pre_a2_table_is_void_because_it_picks_the_wrong_underlay() {
        for (page, name) in pages() {
            let old = old_inner_background(page, 0.0);
            let new = inner_ring_background(page, 0.0);
            let old_pick = ring_style_for(old, FAINTEST, &SHIPPED_TONES).halo;
            let new_pick = ring_style_for(new, FAINTEST, &SHIPPED_TONES).halo;
            if old_pick < new_pick {
                // The old table is *optimistic*: it would paint a weaker halo than the raw frame
                // needs, and the ring would fall short where it matters most — in the hole.
                let undersized = pair_contrast(new, CHAIN_RING_RGB, old_pick, FAINTEST);
                assert!(
                    undersized < PAIR_THRESHOLD,
                    "{name}: the old pick {old_pick} happens to work ({undersized:.2}:1)"
                );
            }
            // Every pick the *new* table makes is legible by construction.
            let picked = pair_contrast(new, CHAIN_RING_RGB, new_pick, FAINTEST);
            assert!(
                picked >= PAIR_THRESHOLD,
                "{name}: the new pick {new_pick} gives {picked:.2}:1"
            );
        }
        // A white page is the case that shows it plainly: the masked page said 0.40, the raw frame
        // says 0.45, and at 0.40 the ring is short of the bar.
        assert_eq!(
            ring_style_for(old_inner_background(grey(250), 0.0), FAINTEST, &SHIPPED_TONES).halo,
            0.4
        );
        assert_eq!(
            ring_style_for(inner_ring_background(grey(250), 0.0), FAINTEST, &SHIPPED_TONES).halo,
            0.6
        );
    }

    /// The recomputed table, as numbers. Read down the "shipped tone, no halo" column: the raw frame
    /// spans L 0.005–0.956, so the ring alone is anything from 1.1:1 to 6.5:1 — the range A4 exists to
    /// cover, and the reason the ladder is coarse (five steps, not a continuum).
    #[test]
    fn the_new_table_over_the_whole_luminance_range() {
        let expected: [(Rgb, f32, f32); 6] = [
            // page, pair at halo 0, picked halo
            (grey(250), 1.25, 0.6),
            (grey(230), 1.11, 0.6),
            (grey(128), 2.04, 0.4),
            (grey(90), 3.11, 0.0),
            (grey(30), 6.00, 0.0),
            (Rgb::new(16, 16, 18), 6.49, 0.0),
        ];
        for (page, bare, halo) in expected {
            let observed = pair_contrast(page, CHAIN_RING_RGB, 0.0, FAINTEST);
            assert!(
                (observed - bare).abs() < 0.02,
                "at {page:?} the bare ring is {observed:.2}:1, expected {bare:.2}:1"
            );
            let style = ring_style_for(page, FAINTEST, &SHIPPED_TONES);
            assert!(
                (style.halo - halo).abs() < 1e-6,
                "at {page:?} the pick is {}, expected {halo}",
                style.halo
            );
            // …and the pick is the *smallest* one that clears the bar, not merely one that does.
            let smaller: Vec<f32> = HALO_LADDER.iter().copied().filter(|h| *h < halo).collect();
            for candidate in smaller {
                assert!(
                    pair_contrast(page, CHAIN_RING_RGB, candidate, FAINTEST) < PAIR_THRESHOLD,
                    "at {page:?} {candidate} would have been enough — the search is not minimal"
                );
            }
        }
    }

    /// **Where the tone ladder does and does not help** (docs/21 §5.24.11).
    ///
    /// This is the correction a first draft of this module got wrong: the ladder does *not* let a
    /// faint ring skip its underlay, because a faint ring is mostly the page it sits on — at alpha
    /// `0.68` even the brand blue is diluted to 2.2:1 on raw light content. What the ladder buys is
    /// the *strong* rings: at full alpha the same tone clears 3.6:1 with no band at all.
    #[test]
    fn the_tone_ladder_helps_the_strong_rings_and_not_the_faint_ones() {
        let light = grey(230);
        // The faintest ring needs the same underlay with either ladder: the tone cannot carry it.
        assert_eq!(
            ring_style_for(light, FAINTEST, &SHIPPED_TONES).halo,
            0.6,
            "the shipped tone"
        );
        assert_eq!(
            ring_style_for(light, FAINTEST, &PROTOTYPE_TONES).halo,
            0.6,
            "…and no tone in the prototype's ladder does better at this alpha"
        );
        assert!(
            pair_contrast(light, ACCENT_RGB, 0.0, FAINTEST) < PAIR_THRESHOLD,
            "the brand blue is diluted by the page at the ramp's floor"
        );
        // A ring next to the selection is at full alpha, and *there* the darker tone replaces the
        // band: 3.6:1 with nothing under it.
        let strong = ring_style_for(light, 1.0, &PROTOTYPE_TONES);
        assert_eq!(strong.tone, ACCENT_RGB);
        assert_eq!(strong.halo, 0.0);
        assert!(pair_contrast(light, ACCENT_RGB, 0.0, 1.0) >= 3.5);
        // The shipped tone still needs a band even at full alpha (2.6:1 bare).
        assert_eq!(ring_style_for(light, 1.0, &SHIPPED_TONES).halo, 0.6);
    }

    /// The property the sweep is for: whatever background A4 hands in, the decision either clears the
    /// bar or is the strongest thing the ladder can do — and it never picks a halo larger than it
    /// needs. This is the invariant the sampler will rely on.
    #[test]
    fn every_background_gets_the_smallest_underlay_that_works() {
        for value in 0..=255u8 {
            let page = grey(value);
            for tones in [&SHIPPED_TONES[..], &PROTOTYPE_TONES[..]] {
                let style = ring_style_for(page, FAINTEST, tones);
                let achieved = pair_contrast(page, style.tone, style.halo, FAINTEST);
                assert!(
                    achieved >= PAIR_THRESHOLD || style.halo == HALO_LADDER[4],
                    "grey {value}, {tones:?} tones: {style:?} gives {achieved:.2}:1"
                );
                // Monotone in the halo for this tone: if a smaller one worked, it would have won.
                for smaller in HALO_LADDER.iter().copied().filter(|h| *h < style.halo) {
                    let weaker = pair_contrast(page, style.tone, smaller, FAINTEST);
                    assert!(
                        weaker < PAIR_THRESHOLD || smaller == style.halo,
                        "grey {value}: {smaller} with {:?} already gives {weaker:.2}:1",
                        style.tone
                    );
                }
            }
        }
    }

    /// The table, printed — `cargo test --lib ring_contrast_probe -- --ignored --nocapture`
    /// (docs/21 §5.24.11 is this output, so the doc cannot drift from the code).
    #[test]
    #[ignore = "probe: prints the recomputed A4 tables for the docs"]
    fn ring_contrast_probe() {
        println!(
            "[ring-a4] the hole (A2): outer rings are still masked, inner rings are raw + walking wash"
        );
        println!(
            "[ring-a4] {:>14} {:>22} {:>22} {:>22}",
            "page", "outer (masked) bg", "inner (raw) bg", "inner + walk"
        );
        for (page, name) in pages() {
            let outer = outer_ring_background(page);
            let inner = inner_ring_background(page, 0.0);
            let walking = inner_ring_background(page, 1.0);
            println!(
                "[ring-a4] {name:>14} {:>22} {:>22} {:>22}",
                format!("{outer:?} L={:.3}", relative_luminance(outer)),
                format!("{inner:?} L={:.3}", relative_luminance(inner)),
                format!("{walking:?} L={:.3}", relative_luminance(walking))
            );
        }
        println!(
            "[ring-a4] {:>14} {:>16} {:>16} {:>16} {:>16}",
            "page", "bare", "pick (1 tone)", "pick (ladder)", "walk pick"
        );
        for (page, name) in pages() {
            let pick = |background: Rgb, tones: &[Rgb]| {
                let style = ring_style_for(background, FAINTEST, tones);
                format!(
                    "{}@{:.2} ({:.2}:1)",
                    if style.tone == CHAIN_RING_RGB {
                        "ring".to_owned()
                    } else {
                        format!("{:?}", style.tone)
                    },
                    style.halo,
                    pair_contrast(background, style.tone, style.halo, FAINTEST)
                )
            };
            let inner = inner_ring_background(page, 0.0);
            println!(
                "[ring-a4] {name:>14} {:>16} {:>16} {:>16} {:>16}",
                format!("{:.2}:1", pair_contrast(inner, CHAIN_RING_RGB, 0.0, FAINTEST)),
                pick(inner, &SHIPPED_TONES),
                pick(inner, &PROTOTYPE_TONES),
                pick(inner_ring_background(page, 1.0), &SHIPPED_TONES)
            );
        }
    }
}
