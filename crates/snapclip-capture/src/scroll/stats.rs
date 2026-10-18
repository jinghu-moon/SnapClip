//! What the stitcher actually did with each frame, as counts rather than a log line per frame.
//!
//! A throttled log answers "what did the interesting frames look like" and, by construction,
//! nothing else: it prints on state change, so the frames that carried no news are the ones it
//! never shows. That difference is not academic. A real capture of 80 882 rows ran 11 344 frames
//! and every published `dy` was 93, which arithmetic says cannot be true — 11 344 shifts of 93
//! rows is a million rows against a canvas of 80 882 — so most frames carried no movement and the
//! log simply never said so (docs/39).
//!
//! This counter is the answer to such questions without a rebuild: one line at the end of a
//! session, with every placement counted and the total distance the estimator reported. It is
//! measurement only and does not exist in the product; `docs/35 §19` and `docs/39 §3` describe how
//! it is enabled and what it prints.

use crate::scroll::estimate::Estimate;
use crate::scroll::stitch::Placement;

/// Per-session placement counts plus the movement the estimator reported.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PlacementStats {
    appended: u64,
    contained: u64,
    prepended: u64,
    full: u64,
    /// Frames the estimator could not place at all; not a [`Placement`], which is why it is
    /// counted here rather than derived from one.
    unknown: u64,
    /// Sum of `|dy|` over the frames that had a shift, in canvas pixels.
    moved_px: u64,
    /// Frames whose estimate was a shift, whatever happened to the placement afterwards.
    nonzero: u64,
    frames: u64,
}

impl PlacementStats {
    /// Count one measured frame. Every frame of a session goes through here once, in order.
    pub(crate) fn record(&mut self, estimate: &Estimate, placement: Option<&Placement>) {
        self.frames += 1;
        if let Estimate::Shift { dy, .. } = estimate {
            self.moved_px += u64::from(dy.unsigned_abs());
            self.nonzero += 1;
        }
        match placement {
            Some(Placement::Appended { .. }) => self.appended += 1,
            Some(Placement::Prepended { .. }) => self.prepended += 1,
            Some(Placement::Contained) => self.contained += 1,
            Some(Placement::Full) => self.full += 1,
            None => self.unknown += 1,
        }
    }

    /// The closing line: every count, so a session explains itself without the per-frame log.
    pub(crate) fn line(&self) -> String {
        format!(
            "placements: appended={} contained={} prepended={} unknown={} full={} \
             moved_px={} nonzero={} frames={}",
            self.appended,
            self.contained,
            self.prepended,
            self.unknown,
            self.full,
            self.moved_px,
            self.nonzero,
            self.frames,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shift(dy: i32) -> Estimate {
        Estimate::Shift {
            dy,
            votes: 1,
            second: 0,
        }
    }

    #[test]
    fn every_placement_lands_in_its_own_count() {
        let mut stats = PlacementStats::default();
        stats.record(
            &shift(93),
            Some(&Placement::Appended {
                growth: 93,
                band: 404,
            }),
        );
        stats.record(&shift(-60), Some(&Placement::Prepended {
            growth: 60,
            band: 404,
        }));
        stats.record(&shift(7), Some(&Placement::Contained));
        stats.record(&shift(93), Some(&Placement::Full));
        stats.record(
            &Estimate::Unknown {
                best: 0,
                votes: 2,
                second: 2,
                unmatched: 2402,
            },
            None,
        );
        assert_eq!(
            stats.line(),
            "placements: appended=1 contained=1 prepended=1 unknown=1 full=1 \
             moved_px=253 nonzero=4 frames=5"
        );
    }

    /// The number this instrument exists for: a session where most frames moved nothing must not
    /// look like one where every frame moved, even though both publish the same `dy`.
    #[test]
    fn a_long_quiet_stretch_is_visible_as_counts() {
        let mut stats = PlacementStats::default();
        stats.record(
            &shift(93),
            Some(&Placement::Appended {
                growth: 93,
                band: 404,
            }),
        );
        for _ in 0..12 {
            stats.record(
                &Estimate::Unknown {
                    best: -558,
                    votes: 2,
                    second: 2,
                    unmatched: 2402,
                },
                None,
            );
        }
        assert_eq!(
            stats.line(),
            "placements: appended=1 contained=0 prepended=0 unknown=12 full=0 \
             moved_px=93 nonzero=1 frames=13"
        );
    }

    #[test]
    fn a_negative_shift_counts_as_distance() {
        let mut stats = PlacementStats::default();
        stats.record(&shift(-124), Some(&Placement::Contained));
        assert!(stats.line().contains("moved_px=124"));
    }
}
