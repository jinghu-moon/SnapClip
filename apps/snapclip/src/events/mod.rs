//! The shell's typed event channel (docs/23 T4.5).
//!
//! One shape travels here — [`AppEvent`], which carries identity, state, size and an
//! artifact reference, never pixels — and one rule governs it: a consumer drops anything
//! whose `generation` is not newer than what it has already accepted. The generation is
//! stamped by the bus at publish time, so no producer has to own a counter and no consumer
//! has to know which producer spoke.
//!
//! Why a channel and not a GPUI `Entity` event: producers are ordinary threads (the
//! clipboard listener, capture's export worker). `AsyncApp` is `Rc`-based and cannot leave
//! the UI thread, so the crossing has to be a `Send` channel; the GPUI side consumes it in a
//! foreground task and touches entities only inside `Entity::update`.
//!
//! This module is deliberately framework-free: it is the piece that must be unit-testable
//! without a window, and the piece that stays once the Tauri host is gone (P6).

use std::sync::{Arc, Mutex, atomic::AtomicU64, atomic::Ordering};

use async_channel::{Receiver, Sender, TryRecvError};
use snapclip_model::AppEvent;

/// Publishes [`AppEvent`]s to every subscriber.
///
/// Clone-able and cheap: adapters hold a clone each, and the bus itself is what keeps the
/// fan-out and the generation counter in one place.
#[derive(Clone)]
pub struct EventBus {
    subscribers: Arc<Mutex<Vec<Sender<AppEvent>>>>,
    next_generation: Arc<AtomicU64>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(Vec::new())),
            next_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Start listening. The stream stops when it is dropped; a dead subscriber is pruned
    /// on the next publish rather than kept forever.
    pub fn subscribe(&self) -> EventStream {
        let (sender, receiver) = async_channel::unbounded();
        self.subscribers
            .lock()
            .expect("event subscriber list is never poisoned: only Vec pushes happen")
            .push(sender);
        EventStream {
            receiver,
            gate: GenerationGate::default(),
        }
    }

    /// Stamp `event` with the next generation and deliver it. Returns the generation.
    ///
    /// The send is non-blocking: the channel is unbounded because these events are
    /// low-frequency by construction, and a UI that cannot keep up should render a stale
    /// list rather than block a capture thread.
    pub fn publish(&self, event: AppEvent) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let event = event.with_generation(generation);
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("event subscriber list is never poisoned: only Vec pushes happen");
        subscribers.retain(|subscriber| subscriber.try_send(event.clone()).is_ok());
        generation
    }

    /// How many events have been published so far.
    pub fn published(&self) -> u64 {
        self.next_generation.load(Ordering::Relaxed)
    }
}

/// One subscriber's view of the bus.
pub struct EventStream {
    receiver: Receiver<AppEvent>,
    gate: GenerationGate,
}

impl EventStream {
    /// The next event that is newer than everything this stream has accepted.
    ///
    /// `None` means the bus is gone; stale events are skipped, not returned.
    pub async fn next(&mut self) -> Option<AppEvent> {
        loop {
            let event = self.receiver.recv().await.ok()?;
            if self.gate.accept(event.generation()) {
                return Some(event);
            }
        }
    }

    /// Same rule as [`Self::next`], without waiting. Used by tests and by code that wants to
    /// drain what is already queued.
    pub fn try_next(&mut self) -> Option<AppEvent> {
        loop {
            match self.receiver.try_recv() {
                Ok(event) => {
                    if self.gate.accept(event.generation()) {
                        return Some(event);
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Closed) => return None,
            }
        }
    }

    /// The newest generation this stream has accepted, or `0` before the first event.
    pub fn last_generation(&self) -> u64 {
        self.gate.last_accepted()
    }
}

/// Drops events that are not newer than the last one accepted.
///
/// This is the whole staleness discipline, in one place: an event that arrives late (a
/// capture session superseded by a newer one, a reordered clipboard notification) cannot
/// move the UI backwards.
#[derive(Debug, Default)]
pub struct GenerationGate {
    last_accepted: u64,
}

impl GenerationGate {
    pub fn accept(&mut self, generation: u64) -> bool {
        if generation > self.last_accepted {
            self.last_accepted = generation;
            true
        } else {
            false
        }
    }

    pub fn last_accepted(&self) -> u64 {
        self.last_accepted
    }
}

#[cfg(test)]
mod tests {
    use super::{EventBus, GenerationGate};
    use snapclip_model::{AppEvent, ClipboardEvent, PayloadKind};

    fn clipboard_event(clip_id: &str) -> AppEvent {
        AppEvent::Clipboard(ClipboardEvent {
            clip_id: clip_id.into(),
            kind: PayloadKind::Text,
            dimensions: None,
            pixel_format: None,
            // Producers do not own the counter; the bus stamps over this.
            generation: 0,
        })
    }

    #[test]
    fn the_bus_stamps_each_event_with_a_newer_generation() {
        let bus = EventBus::new();
        assert_eq!(bus.published(), 0);
        assert_eq!(bus.publish(clipboard_event("a")), 1);
        assert_eq!(bus.publish(clipboard_event("b")), 2);
        assert_eq!(bus.published(), 2);
    }

    #[test]
    fn every_subscriber_sees_every_event_in_order() {
        let bus = EventBus::new();
        let mut first = bus.subscribe();
        let mut second = bus.subscribe();
        bus.publish(clipboard_event("a"));
        bus.publish(clipboard_event("b"));

        // One subscriber draining is not allowed to steal from the other: each has its own
        // queue, which is the fan-out the tray and the history screen both need.
        for stream in [&mut first, &mut second] {
            let mut ids = Vec::new();
            while let Some(event) = stream.try_next() {
                match event {
                    AppEvent::Clipboard(event) => ids.push(event.clip_id),
                    other => panic!("unexpected event: {other:?}"),
                }
            }
            assert_eq!(ids, ["a".to_string(), "b".to_string()]);
        }
    }

    #[test]
    fn a_subscriber_that_appears_late_does_not_receive_history() {
        let bus = EventBus::new();
        bus.publish(clipboard_event("old"));
        let mut late = bus.subscribe();
        assert!(late.try_next().is_none());
        bus.publish(clipboard_event("new"));
        assert!(matches!(late.try_next(), Some(AppEvent::Clipboard(event)) if event.clip_id == "new"));
    }

    #[test]
    fn the_gate_drops_repeats_and_stale_events_only() {
        let mut gate = GenerationGate::default();
        // Before anything is accepted, generation 0 is not new information.
        assert!(!gate.accept(0));
        assert!(gate.accept(1));
        assert!(!gate.accept(1), "the same generation twice is not new");
        assert_eq!(gate.last_accepted(), 1);
        assert!(!gate.accept(0), "a stale event cannot move the UI backwards");
        assert!(gate.accept(2));
        assert_eq!(gate.last_accepted(), 2);
    }

    #[test]
    fn a_stream_skips_a_stale_event_and_reports_the_newest_it_accepted() {
        let bus = EventBus::new();
        let mut stream = bus.subscribe();
        bus.publish(clipboard_event("first"));
        bus.publish(clipboard_event("second"));
        assert!(matches!(stream.try_next(), Some(AppEvent::Clipboard(event)) if event.clip_id == "first"));
        assert_eq!(stream.last_generation(), 1);
        assert!(matches!(stream.try_next(), Some(AppEvent::Clipboard(event)) if event.clip_id == "second"));
        assert_eq!(stream.last_generation(), 2);
        assert!(stream.try_next().is_none(), "an empty stream hands out nothing");
    }
}
