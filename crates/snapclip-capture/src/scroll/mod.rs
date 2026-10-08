//! The scroll-capture subsystem (`docs/30` §28 lists its eleven files: ten production files and
//! the test-only `testkit.rs`).
//!
//! What is here today:
//!
//! * `testkit` (`P1.01`) — the synthetic fixture every later `P1` task is judged against.
//! * `perf_probe` (`P0.03`) — the measurement device for `E-PERF-1`, kept because its numbers are
//!   the only matching-cost data this repository has and `P1.02`+ must re-run it.
//!
//! Both are `#[cfg(test)]`: the estimator itself arrives with `P1.02`, and this module is a real
//! (non-test) module from `P1.01` on — see `lib.rs` and `DEV-8`.
//!
//! Gate note (`docs/30` §28.4): nothing under `scroll/` may reference `windows`, `sampler` or any
//! platform FFI. Everything here is pure arithmetic over byte buffers, and the second scan in
//! `tools/check-dependency-direction.ps1` (`P6.07`) will make that mechanical.

#[cfg(test)]
mod perf_probe;

#[cfg(test)]
mod testkit;
