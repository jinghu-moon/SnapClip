//! The scroll-capture subsystem (`docs/30` §28 lists its files: eleven production files and the
//! test-only `testkit.rs`).
//!
//! What is here today:
//!
//! * `observation` (`P1.02`) — `Axis`, the one place the horizontal case is expressed.
//! * `displacement` (`P1.04`) — what the estimator is allowed to answer and what the session is
//!   allowed to do with the answer. Carries the three layers (`P1.05`–`P1.07`), the four gates
//!   (`P1.08`–`P1.11`), the prior (`P1.14`) and the `Scratch` buffers (`P1.13`).
//! * `canvas` (`P1.17`) — the recovered image, its coverage map, the bounded band store
//!   (`P1.18`–`P1.22`) and `ViewportState`, which is where a confirmed step becomes a write.
//! * `orb` (`P1.23`) — the conditionally triggered second opinion. It can only answer
//!   `Agree`/`Disagree`/`NoEvidence`; it never produces a reported displacement.
//! * `loop_control` (`P3.03`) — the closed loop's pieces: the watchdog that switches transports, the
//!   settle window, the `ĝ` controller, and (from `P3.09`) the driver that runs them in order.
//! * `preview` (`P3.08`) — the capacity-1 preview mailbox the overlay thread drains (§19.3).
//! * `session` (`P3.04`) — the aggregate root: the axis, the canvas, the step counter, the streak,
//!   the stop promise, and the command port (§27.2).
//! * `ports` (`P3.09`) — the two platform seams, `FrameSource` and `ScrollActuator` (§27.1).
//! * `export` (`P4.01`) — the third seam, and the one the **shell** fills rather than the platform:
//!   rows go to an encoder a band at a time, and the height is a precondition of the first row
//!   rather than something discovered while writing (§17.7).
//! * `testkit` (`P1.01`) — the synthetic fixture every later `P1` task is judged against.
//! * `acceptance` (`P1.24`) — `E-ACC-1` as a gate: the `§29.3` scan over the whole funnel, judged by
//!   byte equality with the generator's truth. Test-only, but it is the reason G1 is checkable.
//! * `perf_probe` (`P0.03`) — the measurement device for `E-PERF-1`, kept because its numbers are
//!   the only matching-cost data this repository has and `P1.05`+ must re-run it on the real
//!   layer 1 (see `docs/30` §23.3.1 and `DEV-8`).
//!
//! `observation`, `displacement`, `canvas` and `orb` are production code; `testkit` and
//! `perf_probe` are `#[cfg(test)]`. This module became a real (non-test) module with `P1.01` — see
//! `lib.rs` and `DEV-8`.
//!
//! Gate note (`docs/30` §28.4): nothing under `scroll/` may reference `windows`, `sampler` or any
//! platform FFI. Everything here is pure arithmetic over byte buffers, and the second scan in
//! `tools/check-dependency-direction.ps1` (`P6.07`) will make that mechanical.

pub(crate) mod canvas;

pub(crate) mod displacement;

/// `export` (`P4.01`) — the seam the shell fills: rows go to an encoder one band at a time, and the
/// height is a precondition of the first row rather than something discovered while writing
/// (§17.7). It sits beside `ports` rather than inside it because `ports` is the **platform** seam
/// (Windows implements it) while this one is the **output format** seam (`DEV-2`).
///
/// Unlike every other module here this one is `pub`, and `P4.02` is why: a port the composition root
/// cannot name is not a port. `P4.01` declared it `pub(crate)` and `apps/snapclip` could not have
/// implemented `RowBandSink` at all — the visibility widening is the fix, not a convenience.
pub mod export;

pub(crate) mod loop_control;

pub(crate) mod observation;

/// [`Axis`] is `pub` for the same reason `export` is: it is the type of `ImageMeta::axis`, so a
/// caller outside this crate cannot build or read that field without naming it.
pub use observation::Axis;

pub(crate) mod orb;

/// `ports` (`P3.09`) — the platform seams: what the driver reads frames from and injects through.
/// The vocabulary lives here and the implementations live in `windows/`, which is the only shape
/// that satisfies §28.4's "nothing under `scroll/` may reference the Windows module" while the driver
/// itself stays in `scroll/loop_control.rs` (§28.2).
pub(crate) mod ports;

pub(crate) mod preview;

pub(crate) mod session;

/// [`ScrollDiagnosticCode`] is `pub` for §27.1's reason rather than for convenience: it is listed
/// there among the types that cross the crate boundary, and it is what the shell names when it
/// reports why an export was trimmed. The module that owns it has to stay `pub(crate)` — it also
/// owns [`session::ScrollSession`], which §27.1 keeps internal — so the re-export is the only shape
/// that gives the vocabulary a public path without publishing the session (the [`Axis`] precedent
/// above).
pub use session::ScrollDiagnosticCode;

#[cfg(test)]
mod acceptance;

#[cfg(test)]
mod perf_probe;

#[cfg(test)]
mod testkit;
