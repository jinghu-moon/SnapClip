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
//!   that dropped updates on a timer would be making a policy decision inside a data structure.
//!
//! ## Not here yet
//!
//! * `PreviewSink` (§27.1 lists it as a seam trait). Its first consumer is the driver's preview
//!   publishing in `P5.01`; a trait with no implementor and no test double is dead code today.
//! * The windowed thumbnail derivation (`scale`, §19.2/§19.3) — `P5.02`.

#![allow(dead_code)] // First producer is the driver (`P5.01`); `P3.08` lands the port itself.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

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
                *slot = update;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

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
}
