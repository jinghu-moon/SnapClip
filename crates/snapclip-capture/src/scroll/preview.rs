//! The state channel: `PreviewStream` + `PreviewUpdate` (`docs/30` §19.3; task `P3.08`).
//!
//! ## Why this is not the same shape as the command port
//!
//! §27.2 calls it "一处刻意的非对称设计" and this file is the half that may **lose** things. A
//! preview update is a *state*: the newest one is the whole truth, so dropping the ones in between
//! costs nothing. A command (`stop`, `cancel`, `undo`) is an *intent*: dropping one loses something
//! the user did. The two live in separate types with separate mechanisms because a single channel
//! would force one semantic on both — and it would have to be the wrong one for one of them.
//!
//! Three mechanisms, one for each kind of traffic:
//!
//! | traffic | mechanism | may wait? | may be lost? |
//! |---|---|---|---|
//! | `stop`/`cancel`/`undo`/`shutdown` | sticky bit (`ScrollController`, `session.rs`) | no | **no** |
//! | `set_follow` | capacity-1 slot behind a `Mutex` | yes (nanoseconds) | no |
//! | preview updates | **one slot per kind** behind a `try_lock` | **no** | yes, and `dropped` says so |
//!
//! ## Why one slot per kind and not one slot
//!
//! §19.3 constraint 1 says "capacity 1, newest overwrites oldest", justified by "the preview is
//! **idempotent state**, not an event stream, so losing the intermediate states loses nothing". That
//! justification holds for a stream of **one** kind and fails for four: `Span` (progress),
//! `Viewport` (where the box is) and `Bands` (which rows just became readable) are three different
//! states, and the driver produces all three on the same step. With a single slot, whichever was
//! published last is the only one the consumer ever sees, so the panel's progress row would starve
//! behind the box, or the box behind the pixels — a silent, permanent loss, not an intermediate
//! state.
//!
//! So the slot is per kind. The channel stays **bounded** (four slots, one per variant) and stays
//! **lossy within a kind** — the newest `Span` replaces the previous unseen `Span`, which is what
//! "latest wins" was for, and `dropped` still counts only updates a consumer never got to see.
//!
//! ## The one kind that is not a state
//!
//! `P5.01` found the limit of "latest wins": three of the four kinds are states, and `Bands` is not.
//! A state's newest value is the whole truth; a **delta**'s newest value is not, because the rows an
//! earlier delta announced did not stop being readable. Replacing an unseen `Bands` therefore loses
//! rows rather than information about rows — and it loses them for good, because the announcement is
//! what a consumer derives a thumbnail from.
//!
//! So the unseen `Bands` is widened instead ([`merge`]), and only that kind is. This is a decision
//! about what "latest" means for a delta, not a rate limit: the rate limit is still the producer's
//! ([`PreviewCadence`]), and it is why the producer also has to coalesce its own suppressed window
//! (`ScrollDriver::publish_bands`).
//!
//! | traffic | mechanism | may wait? | may be lost? |
//! |---|---|---|---|
//! | `stop`/`cancel`/`undo`/`shutdown` | sticky bit (`ScrollController`, `session.rs`) | no | **no** |
//! | `set_follow` | capacity-1 slot behind a `Mutex` | yes (nanoseconds) | no |
//! | preview updates | **one slot per kind** behind a `try_lock` | **no** | yes, and `dropped` says so |
//! | ... of which `Bands` | one slot, **widened** while unseen | no | only across `scale` |
//!
//! ## What this port deliberately does not do
//!
//! * **It does not wake anyone.** The consumer is the overlay thread's Win32 message loop, and the
//!   wake is a window message (`P3.07`'s `SCROLL_READY_MESSAGE`, `WM_APP + 45`), posted by whoever
//!   published. §19.3's sketch has a `Condvar` here; a `Condvar` nobody waits on is a second wake
//!   mechanism that can silently disagree with the first, and it would also let this file assume
//!   the consumer is a thread that can block — which the message loop cannot.
//! * **It does not carry pixels** (§19.3 constraint 2). The update says *which* rows became
//!   readable; the pixels are read through the band store's read-only handle. That keeps bulk memory
//!   out of a `Mutex` and keeps §17.5's bands from being copied.
//! * **It does not rate-limit.** §19.3's 10 Hz ceiling belongs to the producer (`P5.01`): a port
//!   that dropped updates on a timer would be making a policy decision inside a data structure. The
//!   limit is [`PreviewCadence`], and the producer consults it before it calls [`PreviewStream`].
//!
//! ## Where the thumbnail is
//!
//! [`refresh_window`] derives the windowed thumbnail (`P5.02`) and puts it in the **canvas' own**
//! [`crate::scroll::canvas::BandStore`] as a band at a scale — not in a store of its own. ADR-9 and
//! §19.2: the thumbnail is the same data at a different scale, so it shares the budget, the LRU and
//! §22.3's eviction order, in which the rebuildable half is spent first. A separate store would need a
//! separate budget, and two budgets cannot be spent in that order.
//!
//! ## Not here yet
//!
//! * `PreviewSink` (§27.1 lists it as a seam trait). `P5.01` is where it would have been used and it
//!   is still not here: the driver's only publisher is `PreviewStream`, and a real `PreviewStream` is
//!   cheap to construct and drain in a test, so a trait over it would have one implementor and no
//!   double. §27.1 lists it as a boundary type; the boundary it names is already crossed by
//!   `PreviewUpdate`, which is the type that has to stay small and pixel-free.
//! * The consumer that calls [`refresh_window`] — `P5.03`'s overlay, which asks for the window it is
//!   about to draw.

#![allow(dead_code)] // First producer is the driver (`P5.01`); `P3.08` lands the port itself.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::scroll::canvas::{Band, BandError, RecoveredImage};
use crate::scroll::displacement::Status;
use crate::scroll::session::StopReason;

/// One latest-only piece of preview state (`docs/30` §19.3).
///
/// `Copy` is load-bearing rather than convenient: §19.3 constraint 2 says an update carries *where*
/// the pixels are, never the pixels. A type that could hold a `Vec<u8>` would compile just as well
/// and would silently move megabytes through the mailbox — the size assertion in this module's tests
/// is what makes the constraint mechanical.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PreviewUpdate {
    /// Rows `first_row..first_row + rows` became readable at `scale` (1 = full resolution).
    Bands { first_row: u64, rows: u32, scale: u32 },
    /// Progress, without pixels: §19.5's row is "`primary_len` + 步数 + 累计未采用步数". All three
    /// are here because the panel cannot read the session — that is the entire reason a port exists.
    Span {
        primary_len: u64,
        steps: u32,
        discarded: u32,
    },
    /// Where the viewport box is and what the last step did (§19.4's three appearances).
    Viewport { band: u64, status: Status },
    /// The session is over; the box stops moving and the reason is shown (§19.4, §20.4).
    Ended { reason: StopReason },
}

impl PreviewUpdate {
    /// Which slot this belongs in — one slot per kind, so publishing three kinds on one step cannot
    /// make them evict each other (see the module doc).
    ///
    /// A discriminant rather than a `match` at each use site: `Mailbox::push` needs "is there already
    /// one of these?", and a caller that forgot a variant would silently share a slot with another.
    fn kind(&self) -> Kind {
        match self {
            Self::Bands { .. } => Kind::Bands,
            Self::Span { .. } => Kind::Span,
            Self::Viewport { .. } => Kind::Viewport,
            Self::Ended { .. } => Kind::Ended,
        }
    }
}

/// The four kinds, as a value that can be compared.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bands,
    Span,
    Viewport,
    Ended,
}

/// At most one unseen update per kind, oldest first.
///
/// Bounded by construction: four kinds, so four entries. The `VecDeque` is not a queue that can grow
/// — [`Self::push`] replaces rather than appends whenever its kind is already present.
#[derive(Default)]
struct Mailbox {
    slots: VecDeque<PreviewUpdate>,
}

impl Mailbox {
    /// Returns whether an update a consumer had not seen was superseded.
    fn push(&mut self, update: PreviewUpdate) -> bool {
        let kind = update.kind();
        match self.slots.iter_mut().find(|slot| slot.kind() == kind) {
            Some(slot) => {
                *slot = merge(*slot, update);
                true
            }
            None => {
                self.slots.push_back(update);
                false
            }
        }
    }

    fn pop(&mut self) -> Option<PreviewUpdate> {
        self.slots.pop_front()
    }
}

/// What "the newest one wins" means for the one kind that is not a state.
///
/// `Span`, `Viewport` and `Ended` are states: the newest one is the whole truth, which is the
/// justification §19.3 constraint 1 gives for a single slot. `Bands` is a **delta** — it announces
/// that rows became readable — and the newest delta is *not* the whole truth, because the rows an
/// earlier delta announced do not stop being readable. Replacing it would tell the consumer about rows
/// 1980..2520 and never about 0..1980, permanently: a thumbnail is derived from an announcement, and a
/// lost announcement is a row nobody will ever derive.
///
/// So the unseen `Bands` is **widened** to cover both rather than replaced. Widening is the union
/// because the announced set is a prefix of the canvas: the driver announces the origin viewport and
/// from then on only extends the end (or resets it to the whole canvas after a prepend), so two
/// announcements can only overlap or touch.
///
/// Two different `scale`s cannot be expressed as one announcement, so the newer one wins and the push
/// is counted as a supersession — a loss that is visible in `dropped` rather than silent. Nothing
/// publishes more than one scale today (`scale` is 1 everywhere until `P5.02` derives thumbnails); the
/// case is handled rather than assumed away because assuming it away is how it would go unnoticed.
fn merge(previous: PreviewUpdate, update: PreviewUpdate) -> PreviewUpdate {
    match (previous, update) {
        (
            PreviewUpdate::Bands {
                first_row: a,
                rows: a_rows,
                scale: a_scale,
            },
            PreviewUpdate::Bands {
                first_row: b,
                rows: b_rows,
                scale: b_scale,
            },
        ) if a_scale == b_scale => {
            let first_row = a.min(b);
            let end = (a + u64::from(a_rows)).max(b + u64::from(b_rows));
            PreviewUpdate::Bands {
                first_row,
                rows: (end - first_row).min(u32::MAX as u64) as u32,
                scale: a_scale,
            }
        }
        (_, update) => update,
    }
}

/// The driver → overlay channel: one slot per kind, latest wins within a kind, **never blocks the
/// producer** (§19.3, as amended in the module doc).
///
/// `Send + Sync` because the two ends are on different threads; the tests assert it so that a future
/// field that is neither cannot quietly turn this into a thread-affine type.
pub(crate) struct PreviewStream {
    mailbox: Mutex<Mailbox>,
    dropped: AtomicU64,
}

impl PreviewStream {
    pub(crate) fn new() -> Self {
        Self {
            mailbox: Mutex::new(Mailbox::default()),
            dropped: AtomicU64::new(0),
        }
    }

    /// Publish, or drop and count (§19.3 constraint 5).
    ///
    /// `try_lock`, not `lock`: G1's correctness outranks G7's performance, and a preview that is
    /// slow to consume must not be able to slow the capture down. Returning `()` is the point —
    /// §27.4 lists this among the three interfaces that deliberately do not use `Result`, because
    /// "dropped" is a normal outcome rather than an error.
    pub(crate) fn publish(&self, update: PreviewUpdate) {
        match self.mailbox.try_lock() {
            Ok(mut mailbox) => {
                if mailbox.push(update) {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Read the oldest update a consumer has not seen, if there is one.
    ///
    /// Oldest first rather than newest: with a slot per kind there is no "newest" across kinds, and
    /// handing back the kinds in the order they were produced is what lets a consumer that drains the
    /// channel see all of them. Within a kind the slot holds the newest, which is where "latest wins"
    /// lives.
    ///
    /// Poisoning is recovered rather than propagated: the critical section is a move of `Copy` values
    /// and cannot panic, so a poisoned lock still holds valid updates — and panicking on the overlay
    /// thread would take the user's screenshot down with it.
    pub(crate) fn take(&self) -> Option<PreviewUpdate> {
        self.mailbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop()
    }

    /// How many updates were dropped (G12: a silent loss is not visible, a counted one is).
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// §19.3 constraint 4's ceiling, as a **starting value rather than a measurement**.
///
/// The number is derived from the requirement, not from a benchmark: the preview is for a human
/// watching a page scroll, and §19.3 says the eye does not need a finer grain than this. It is marked
/// calibratable on purpose — `E-PERF-4` (§23.1) is the experiment that decides between 5, 10, 20 and
/// 30 Hz, and a constant that can be calibrated has to live where a calibration can reach it. So the
/// thing a calibration changes is *this* line, which is why the rate is spelled in hertz the way the
/// parameter table spells it rather than as a pre-divided millisecond count.
pub(crate) const PREVIEW_HZ: u32 = 10;

/// The same ceiling as the interval the cadence actually compares against.
///
/// Derived rather than written twice: two constants that have to agree is two constants that can
/// disagree, and this pair would disagree silently — a `PREVIEW_HZ` of 20 with an interval of 100 ms
/// looks like a 20 Hz limit and behaves like a 10 Hz one.
pub(crate) const PREVIEW_INTERVAL: Duration = Duration::from_millis(1_000 / PREVIEW_HZ as u64);

/// The producer's rate limit: at most one published update per `interval` (§19.3 constraint 4).
///
/// It lives here rather than inside the port because §19.3.1 裁决 4 puts the limit on the *producer*
/// — "a port that dropped updates on a timer would be making a policy decision inside a data
/// structure" — and the producer is the only thing that knows how much work it just created for the
/// consumer.
///
/// The clock is a parameter for the same reason `Settle`'s is: a rate limit tested against the wall
/// clock is a flaky test, and one that cannot be tested is a claim rather than a mechanism.
pub(crate) struct PreviewCadence {
    interval: Duration,
    last: Option<Instant>,
}

impl PreviewCadence {
    pub(crate) fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    pub(crate) fn interval(&self) -> Duration {
        self.interval
    }

    /// Whether an update may go out at `now`. Stamps the window when it answers yes.
    ///
    /// The first call is always allowed: before it there is nothing that could have been suppressed,
    /// and the consumer has never been told anything. A clock that went backwards reads as "not yet"
    /// rather than as a panic — `Instant::duration_since` saturates, and the settle rule above relies
    /// on the same behaviour.
    pub(crate) fn allow(&mut self, now: Instant) -> bool {
        match self.last {
            Some(last) if now.duration_since(last) < self.interval => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

/// How wide a thumbnail is allowed to be, in physical pixels (§3.8: "跨轴固定 128 物理像素").
pub(crate) const PREVIEW_TARGET_PX: u64 = 128;

/// The downscaling factor that puts a `cross_len`-wide canvas at or under [`PREVIEW_TARGET_PX`].
///
/// Rounding **up** is the whole of it: a scale that rounded down would make the thumbnail wider than
/// the number §3.8 names, and the point of the fixed width is that the preview's cost is a property
/// of the display rather than of the window being captured.
pub(crate) fn preview_scale(cross_len: u64) -> u32 {
    cross_len.div_ceil(PREVIEW_TARGET_PX).max(1) as u32
}

/// The whole thumbnail rows inside `[first_row, first_row + rows)`, as `(start, end)` in canvas rows.
///
/// The bounds are pulled **inwards** to multiples of the scale, because a thumbnail row is made of a
/// whole `scale × scale` block: a band that started mid-block could not be compared with, or dropped
/// interchangeably with, the ones around it. `None` when the request is narrower than one thumbnail
/// row, which is not an error — there is simply nothing at this scale to show.
pub(crate) fn window_bounds(scale: u32, first_row: u64, rows: u64) -> Option<(u64, u64)> {
    let scale = u64::from(scale.max(1));
    let start = first_row.div_ceil(scale) * scale;
    let end = (first_row + rows) / scale * scale;
    (end > start).then_some((start, end))
}

/// One box filter: `canvas_rows` at full resolution in, the same content at `scale` out.
///
/// `canvas_rows` has to cover a whole number of thumbnail rows — `refresh_window` takes the bounds
/// from [`window_bounds`], which is what guarantees it. The cross axis is divided by the same integer
/// the rest of the derivation uses, so a canvas whose width is not a multiple of the scale loses its
/// last few columns rather than producing a band whose row width disagrees with
/// `canvas::band_row_bytes`.
pub(crate) fn thumbnail_rows(canvas_rows: &[u8], cross_len: u64, scale: u32) -> Vec<u8> {
    let step = scale.max(1) as usize;
    let cross = cross_len as usize;
    let canvas_row_bytes = cross * 4;
    assert!(
        step > 0
            && canvas_row_bytes > 0
            && canvas_rows.len() % (canvas_row_bytes * step) == 0,
        "a thumbnail needs whole rows: {} bytes at {cross} px do not divide into {} thumbnail rows",
        canvas_rows.len(),
        step
    );
    let own_rows = canvas_rows.len() / (canvas_row_bytes * step);
    let preview_px = cross / step;
    let mut out = vec![0u8; own_rows * preview_px * 4];
    for row in 0..own_rows {
        for column in 0..preview_px {
            let mut sums = [0u32; 4];
            for dy in 0..step {
                let canvas_row = row * step + dy;
                for dx in 0..step {
                    let offset = (canvas_row * cross + column * step + dx) * 4;
                    for (channel, sum) in sums.iter_mut().enumerate() {
                        *sum += u32::from(canvas_rows[offset + channel]);
                    }
                }
            }
            let count = (step * step) as u32;
            let out_offset = (row * preview_px + column) * 4;
            for (channel, sum) in sums.iter().enumerate() {
                out[out_offset + channel] = (sum / count) as u8;
            }
        }
    }
    out
}

/// Rebuilds the preview's thumbnail for the window `[first_row, first_row + rows)`: §19.2's
/// windowed virtual preview.
///
/// Only the window is ever derived — there is no path here that would produce a whole-canvas
/// thumbnail — and the band it produces is an ordinary [`Band`] at `scale`, inserted into the very
/// store that holds the canvas. That is ADR-9: one budget, one LRU, one eviction order, with §22.3
/// spending the rebuildable half first.
///
/// The previous preview bands go first, always: the thumbnail follows the window, so the window that
/// was there a moment ago is stale by definition, and a band that is not dropped would either overlap
/// the new one (invariant 6) or hold rows nobody asked for. Returns how many thumbnail rows are
/// resident for the window afterwards.
///
/// It deliberately does **not** call `relieve`: `insert` never evicts, and the step that drove this
/// window already called it once (§17.5 ③).
pub(crate) fn refresh_window(
    canvas: &mut RecoveredImage,
    scale: u32,
    first_row: u64,
    rows: u64,
) -> Result<u64, BandError> {
    canvas.bands_mut().drop_previews();
    let Some((start, end)) = window_bounds(scale, first_row, rows) else {
        return Ok(0);
    };
    let canvas_rows = canvas.rows(start, end - start)?;
    let thumbnail = thumbnail_rows(&canvas_rows, canvas.cross_len(), scale);
    canvas
        .bands_mut()
        .insert(Band::at_scale(start, scale, thumbnail));
    Ok((end - start) / u64::from(scale.max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;
    use crate::scroll::canvas::{MemoryBudget, RecoveredImage, StepTally};
    use crate::scroll::observation::{Axis, Observation};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// §19.3 constraint 4: the ceiling is 10 Hz, and it is a ceiling on **updates published**, not on
    /// steps taken (F-08 — the preview must not be the most expensive consumer).
    ///
    /// The clock is a parameter and the steps are synthetic, so this is arithmetic rather than a race:
    /// a hundred steps 5 ms apart span 495 ms, and 10 Hz over 495 ms is four intervals plus the one
    /// that opens the window. A cadence that let every step through would answer 100 here.
    #[test]
    fn updates_are_capped_at_ten_hertz() {
        // §19.3 constraint 4 names the rate, and the parameter table names the constant. If either is
        // recalibrated, this is the line that says the document has to be recalibrated with it.
        assert_eq!(PREVIEW_HZ, 10, "§19.3 constraint 4's starting value");
        assert_eq!(
            PREVIEW_INTERVAL.as_millis(),
            100,
            "…and the interval it implies"
        );

        let mut cadence = PreviewCadence::new(PREVIEW_INTERVAL);
        let start = Instant::now();
        let mut allowed = 0u32;
        for step in 0..100u64 {
            let now = start + Duration::from_millis(step * 5);
            if cadence.allow(now) {
                allowed += 1;
            }
        }

        let duration = Duration::from_millis(99 * 5);
        let ceiling = 1 + duration.as_millis() / PREVIEW_INTERVAL.as_millis();
        assert!(
            u128::from(allowed) <= ceiling,
            "the preview may not be updated more often than {PREVIEW_INTERVAL:?}: \
             {allowed} updates over {duration:?}"
        );
        assert_eq!(
            allowed, 5,
            "the window opens at 0, 100, 200, 300 and 400 ms"
        );
    }

    /// The state channel is allowed to lose updates, but the loss has to be **visible** (G12).
    ///
    /// Both halves matter: `dropped == 99` proves 99 updates were superseded without a consumer, and
    /// the `take` sequence proves the surviving one is the **newest** rather than the oldest (a
    /// queue would have handed back `primary_len: 0` and grown without bound).
    #[test]
    fn preview_updates_may_be_dropped_but_the_count_is_visible() {
        let stream = PreviewStream::new();
        for row in 0..100u64 {
            stream.publish(PreviewUpdate::Span {
                primary_len: row,
                steps: row as u32,
                discarded: 0,
            });
        }

        assert_eq!(stream.dropped(), 99, "one slot, a hundred updates");
        assert_eq!(
            stream.take(),
            Some(PreviewUpdate::Span {
                primary_len: 99,
                steps: 99,
                discarded: 0,
            }),
            "the survivor is the newest, not the first"
        );
        assert_eq!(stream.take(), None, "taking is consuming");

        stream.publish(PreviewUpdate::Ended {
            reason: StopReason::EndReached,
        });
        assert_eq!(
            stream.dropped(),
            99,
            "an empty slot is not a drop: the counter counts supersessions"
        );
        assert_eq!(
            stream.take(),
            Some(PreviewUpdate::Ended {
                reason: StopReason::EndReached
            })
        );
    }

    /// The driver publishes all three of its states on the same step (`P3.09`), and the consumer has
    /// to be able to see all three.
    ///
    /// This is the test that makes the per-kind slot load-bearing rather than tidy: with one slot for
    /// the whole channel, `dropped` would read `2` here and the panel would be missing whichever two
    /// were published first — permanently, because a step publishes each kind once.
    #[test]
    fn one_step_can_publish_every_kind_without_them_evicting_each_other() {
        let stream = PreviewStream::new();
        stream.publish(PreviewUpdate::Span {
            primary_len: 10,
            steps: 1,
            discarded: 0,
        });
        stream.publish(PreviewUpdate::Viewport {
            band: 0,
            status: Status::None,
        });
        stream.publish(PreviewUpdate::Bands {
            first_row: 0,
            rows: 10,
            scale: 1,
        });

        assert_eq!(stream.dropped(), 0, "three kinds, three slots");

        let mut kinds = Vec::new();
        while let Some(update) = stream.take() {
            kinds.push(match update {
                PreviewUpdate::Bands { .. } => "bands",
                PreviewUpdate::Span { .. } => "span",
                PreviewUpdate::Viewport { .. } => "viewport",
                PreviewUpdate::Ended { .. } => "ended",
            });
        }
        assert_eq!(
            kinds,
            ["span", "viewport", "bands"],
            "one of each, in the order the step produced them"
        );
    }

    /// Within a kind, "latest wins" still holds — the per-kind slot is a state, not a queue.
    #[test]
    fn within_a_kind_the_newest_update_still_replaces_the_previous_one() {
        let stream = PreviewStream::new();
        stream.publish(PreviewUpdate::Viewport {
            band: 1,
            status: Status::None,
        });
        stream.publish(PreviewUpdate::Viewport {
            band: 2,
            status: Status::None,
        });

        assert_eq!(stream.dropped(), 1, "the first box was never seen");
        assert_eq!(
            stream.take(),
            Some(PreviewUpdate::Viewport {
                band: 2,
                status: Status::None
            })
        );
        assert_eq!(stream.take(), None);
    }

    /// §27.3's `PreviewStream::publish` row: "**绝不阻塞生产者**".
    ///
    /// `try_lock` and `lock` behave identically in every test where nobody else holds the mutex, so
    /// the only way to tell them apart is to hold it. The consumer here is a stand-in for a slow
    /// overlay thread: the publish must come back **while the lock is held**, and the update must be
    /// *gone* rather than queued. If this ever hangs, the implementation grew a blocking `lock()`
    /// and a slow preview can now stall the capture loop.
    #[test]
    fn a_publish_while_the_consumer_holds_the_lock_is_dropped_not_queued() {
        let stream = PreviewStream::new();
        let guard = stream.mailbox.lock().expect("the mailbox is not poisoned");

        let (tx, rx) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                stream.publish(PreviewUpdate::Span {
                    primary_len: 1,
                    steps: 1,
                    discarded: 0,
                });
                let _ = tx.send(());
            });

            let returned = rx.recv_timeout(Duration::from_millis(500)).is_ok();
            drop(guard);
            assert!(
                returned,
                "publish waited for the consumer to let go: it must use `try_lock` and drop the \
                 update instead (§19.3 constraint 5)"
            );
        });

        assert_eq!(stream.dropped(), 1);
        assert_eq!(
            stream.take(),
            None,
            "a dropped update is dropped, not parked until the consumer comes back"
        );
    }

    /// §19.5's progress row needs three quantities, and the panel cannot read the session.
    #[test]
    fn span_carries_the_progress_the_panel_shows() {
        let update = PreviewUpdate::Span {
            primary_len: 4_096,
            steps: 37,
            discarded: 3,
        };
        let PreviewUpdate::Span {
            primary_len,
            steps,
            discarded,
        } = update
        else {
            panic!("the progress update is its own variant");
        };
        assert_eq!((primary_len, steps, discarded), (4_096, 37, 3));
    }

    /// §19.3 constraint 2, mechanically: no variant can be carrying pixels.
    ///
    /// The largest variant is `Bands` (a `u64` and two `u32`s plus the discriminant). A `Vec`, a
    /// slice or a boxed band would be a pointer here and would pass a looser bound — so the bound is
    /// deliberately tight enough that adding a buffer breaks it and the author has to read §19.3.
    #[test]
    fn an_update_cannot_be_carrying_pixels() {
        assert!(
            std::mem::size_of::<PreviewUpdate>() <= 32,
            "an update says *which* rows became readable; pixels go through the band store"
        );
    }

    /// The port crosses a thread boundary (§27.3's "driver → 覆盖层" row), so this is a contract
    /// rather than a convenience: a field that is neither `Send` nor `Sync` would make the port
    /// unusable from the driver, and the error would surface far away.
    #[test]
    fn the_port_crosses_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PreviewStream>();
        assert_send_sync::<PreviewUpdate>();
    }

    /// §3.8's fixed 128 physical pixels, as arithmetic. A pure function, so the panel's target width
    /// is checked without building a canvas at all — and it is checked from **both** sides: a canvas
    /// at the target needs no downscaling, and one just over it does (rounding up is what keeps the
    /// thumbnail from being *wider* than the number §3.8 names).
    #[test]
    fn the_preview_scale_lands_the_thumbnail_at_its_target_width() {
        assert_eq!(preview_scale(64), 1, "narrower than the target: no room to shrink");
        assert_eq!(preview_scale(128), 1, "exactly the target");
        assert_eq!(preview_scale(129), 2, "one pixel over, and the scale has to move");
        assert_eq!(preview_scale(1_500), 12, "§19.2's worked example: 125 px wide");
        assert_eq!(preview_scale(3_840), 30, "a 4K-wide viewport");
        for cross_len in [64u64, 128, 129, 1_500, 3_840] {
            let scale = preview_scale(cross_len);
            let width = cross_len / u64::from(scale);
            assert!(
                width <= PREVIEW_TARGET_PX,
                "a {cross_len} px canvas at scale {scale} makes a {width} px thumbnail, which is \
                 wider than §3.8's {PREVIEW_TARGET_PX} px target"
            );
        }
    }

    /// §30.6's "预览窗口化（不生成整图缩略）" row (`P5.02`, `docs/30` §19.2 + ADR-5 + ADR-9): the
    /// thumbnail is generated for **the visible window and nothing else**, so what a preview costs is
    /// a function of the window and not of how long the capture has been running.
    ///
    /// The two canvases below differ 10× in length and the first assertion is that the longer one's
    /// thumbnail is the *same size* as the shorter one's. The second says the same thing against the
    /// alternative design: a whole-canvas thumbnail for the 100,000-row canvas is two orders of
    /// magnitude bigger than what is actually held. An implementation that built the whole thumbnail
    /// would fail the first comparison with a 10× gap and the second with a ratio of 1.
    ///
    /// The canvas is narrow (64 px) so the fixture is affordable at 100,000 rows; the scale is
    /// written out rather than derived because [`preview_scale`]'s own test owns the target width.
    /// The numbers are small, the *shape* is the assertion.
    #[test]
    fn a_hundred_thousand_pixel_canvas_never_materialises_a_whole_thumbnail() {
        const CROSS: u64 = 64;
        const EXTENT: u64 = 64;
        const STEP: u64 = 48;
        const SCALE: u32 = 4;
        const WINDOW: u64 = 640;

        let preview_px = CROSS / u64::from(SCALE);
        let preview_row_bytes = preview_px * 4;
        let window_rows = WINDOW / u64::from(SCALE);

        let mut short = scroll_to(CROSS, EXTENT, STEP, 10_000);
        let mut long = scroll_to(CROSS, EXTENT, STEP, 100_000);

        let short_bytes = thumbnail_of(&mut short, SCALE, WINDOW);
        let long_bytes = thumbnail_of(&mut long, SCALE, WINDOW);

        assert_eq!(
            long_bytes,
            window_rows * preview_row_bytes,
            "the thumbnail is one window of {WINDOW} canvas rows at scale {SCALE}: {window_rows} \
             rows of {preview_px} px"
        );
        assert_eq!(
            short_bytes, long_bytes,
            "a 100,000-row canvas spent {long_bytes} B on its thumbnail and a 10,000-row one spent \
             {short_bytes} B — the preview's memory is following the content length"
        );
        assert_eq!(
            long.bands().budget().resident_preview(),
            long_bytes,
            "invariant 7's preview half: the budget has to say what the store holds"
        );

        let whole = (long.primary_len() / u64::from(SCALE)) * preview_row_bytes;
        assert!(
            long_bytes * 10 < whole,
            "the windowed thumbnail is {long_bytes} B and a whole-canvas one would be {whole} B: \
             that is not the two orders of magnitude §19.2 promises"
        );
    }

    /// Scrolls a canvas to at least `target` rows and applies the production eviction rule after
    /// every step, so the fixture is the real store under the real budget — the same shape
    /// `scroll::mem_probe` drives at 1500 px and three lengths.
    fn scroll_to(cross_len: u64, extent: u64, step: u64, target: u64) -> RecoveredImage {
        let mut canvas = RecoveredImage::new(
            Axis::Vertical,
            cross_len,
            MemoryBudget::for_viewport(cross_len, extent),
        );
        let frame = frame(cross_len, extent);
        canvas.start(&frame);
        let mut tally = StepTally {
            step: 0,
            committed: 0,
            discarded: 0,
        };
        while canvas.primary_len() < target {
            canvas.append_confirmed(&frame, step);
            let position = (canvas.primary_len() as i64 - extent as i64).max(0);
            canvas
                .relieve(Some((position, extent)))
                .expect("the budget holds exactly the viewport that is protected");
            tally.step += 1;
            tally.committed += 1;
            canvas.assert_invariants(cross_len, tally);
        }
        canvas
    }

    /// Refreshes the window that ends at the canvas' end and answers what the store now holds for the
    /// preview. Reads the store rather than the derivation's own return value: the claim under test is
    /// about **memory**, and a number the caller was handed is not evidence of one.
    fn thumbnail_of(canvas: &mut RecoveredImage, scale: u32, window: u64) -> u64 {
        let first_row = canvas.primary_len().saturating_sub(window);
        refresh_window(canvas, scale, first_row, window).expect("the canvas has every row the window asks for");
        canvas.bands().resident_preview_bytes()
    }

    /// One reusable viewport of text-like rows. Only its size and its packedness matter here.
    fn frame(cross_len: u64, extent: u64) -> Observation {
        let cross = cross_len as usize;
        let rows = extent as usize;
        let mut pixels = vec![0u8; cross * rows * 4];
        for row in 0..rows {
            for column in 0..cross {
                let offset = (row * cross + column) * 4;
                let value = if (row / 7 + column / 5) % 2 == 0 {
                    0x28u8
                } else {
                    0xE0u8
                };
                pixels[offset] = value;
                pixels[offset + 1] = value;
                pixels[offset + 2] = value;
                pixels[offset + 3] = 0xFF;
            }
        }
        Observation::new(
            pixels,
            Rect::new(0, 0, cross_len as i32, extent as i32),
            0,
            (cross_len as u32, extent as u32),
            Axis::Vertical,
        )
        .expect("the viewport is strictly packed and its region agrees with its size")
    }
}
