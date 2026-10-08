//! The scroll-capture subsystem (`docs/30` §28 lists the eleven files `P1.01` will create
//! here). `P0.03` lands first and contributes only its measurement device, so for now this
//! module is a test-only shell: nothing in it is compiled into the product.
//!
//! Gate note (`docs/30` §28): nothing under `scroll/` may reference `windows` or the
//! platform modules. The `P0.03` device is pure arithmetic over byte buffers and satisfies
//! that already — it must keep doing so when `P1.01` fills the directory.

#[cfg(test)]
mod perf_probe;
