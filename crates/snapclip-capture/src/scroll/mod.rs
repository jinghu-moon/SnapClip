//! The scroll-capture subsystem (`docs/30` §28 lists its eleven files: ten production files and
//! the test-only `testkit.rs`).
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

pub(crate) mod observation;

pub(crate) mod orb;

#[cfg(test)]
mod acceptance;

#[cfg(test)]
mod perf_probe;

#[cfg(test)]
mod testkit;
