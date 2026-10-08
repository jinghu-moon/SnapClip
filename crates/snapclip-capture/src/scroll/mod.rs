//! The scroll-capture subsystem (`docs/30` §28 lists its eleven files: ten production files and
//! the test-only `testkit.rs`).
//!
//! What is here today:
//!
//! * `observation` (`P1.02`) — `Axis`, the one place the horizontal case is expressed.
//! * `testkit` (`P1.01`) — the synthetic fixture every later `P1` task is judged against.
//! * `perf_probe` (`P0.03`) — the measurement device for `E-PERF-1`, kept because its numbers are
//!   the only matching-cost data this repository has and `P1.05`+ must re-run it on the real
//!   layer 1 (see `docs/30` §23.3.1 and `DEV-8`).
//!
//! `observation` is production code; the other two are `#[cfg(test)]`. This module became a real
//! (non-test) module with `P1.01` — see `lib.rs` and `DEV-8`. Later production files arrive with
//! `P1.05` (layer 1) onward.
//!
//! Gate note (`docs/30` §28.4): nothing under `scroll/` may reference `windows`, `sampler` or any
//! platform FFI. Everything here is pure arithmetic over byte buffers, and the second scan in
//! `tools/check-dependency-direction.ps1` (`P6.07`) will make that mechanical.

pub(crate) mod observation;

#[cfg(test)]
mod perf_probe;

#[cfg(test)]
mod testkit;
