//! The canvas is a logical interval, a coverage invariant and a band store — **not a bitmap**
//! (`docs/30` §17.1, §17.2, §20.3; task `P1.17`).
//!
//! `docs/19` §8.1's one genuinely correct constraint survives: for every content pixel there is at
//! least one `Confirmed` observation that wrote it. V2 keeps that constraint and makes it
//! **assertable** — [`RecoveredImage::assert_invariants`] runs after every step (§20.3) — because V1
//! maintained the same idea through a single `next_pos` variable, where any partial commit broke it
//! silently. A rule that is only in the document is a liability; `CaptureState::Adjusting` is the
//! precedent (§5).
//!
//! Three things in here are deliberately **not** what §17.1's sketch says, and each one exists
//! because the sketch cannot make an invariant assertable (`docs/31` §0.6 `DEV-26`):
//!
//! * `CoverageMap` stores **one bit per row**, not just the interval's two ends. Invariant 3 ("the
//!   interval reaches the end") and invariant 4 ("there is no hole inside it") are different facts,
//!   and an interval cannot tell them apart: a write that covers rows 0..500 and 501..1000 leaves
//!   both ends looking right. The ends are therefore *derived* from the set — one source of truth,
//!   because two of them is exactly how a hole becomes invisible.
//! * The invariants are real `assert!`s, not `debug_assert!`s. §17.1 says `debug_assert`; a canvas
//!   with a hole is the one thing this design must never export (§20.3 lists the action for
//!   invariant 4 as "InternalError, and **no export**"), and the checks are O(1) plus one
//!   `O(n log n)` walk over the bands, once per step. Shipping a corrupted canvas because the build
//!   was a release build is the failure mode §20.3 was written to remove.
//! * `RecoveredImage` remembers the thread that created it, which is invariant 8 in its
//!   canvas-level form. `P0.02` measured that the production hand-off really does touch the
//!   immediate `ID3D11DeviceContext` from more than one thread (`docs/30` §21.3); the canvas is the
//!   same kind of object — the scroll driver owns it, the preview gets copies of rows, nobody else
//!   may call into it.
//!
//! `P1.20` adds the fourth: the band store owns the budget. `docs/30` §17.5 asks for an
//! `LruMap<BandIndex, Arc<Band>>` and §22.3 for a `MemoryBudget` whose three fields are all read.
//! The resident side here is a `Vec<ResidentBand>` ordered by position, with a monotonic write tick
//! **inside** each entry, and the bands are plain `Vec<u8>`. Both differences are about keeping one
//! fact in one place — see [`BandStore`] — and the budget being *the store's* property rather than a
//! caller's discipline is what makes `F-07` ("memory does not grow with the image") a property of the
//! code instead of a promise in a document.
//!
//! `P4.06` closes the last open question §17.5 left: who deletes the spill file. The answer is a
//! `Drop` impl and nothing else — §22.7 requires it there rather than on a shutdown path, because a
//! shutdown path is exactly what a panic skips. Two `Drop` impls carry the whole rule and both are
//! greppable: `impl Drop for SpillFile` removes the file, `impl Drop for BandStore` removes the
//! directory it made. There is deliberately **no** `impl Drop for SpillRef` — see [`SpillRef`].
//!
//! Not here yet: the export sink (`P4.01`). Until then the module is exercised by its own tests and
//! the crate's warning budget stays at zero (`docs/31` §4.1).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::geometry::{Point, Rect};
use crate::scroll::displacement::checksum;
use crate::scroll::observation::{Axis, Observation};

/// Bytes per pixel in a band: BGRA, the format every capture path produces (`docs/30` §17.5).
pub(crate) const BYTES_PER_PIXEL: u64 = 4;

/// How much of the viewport a step matches and writes: §17.2's `max(extent/2, extent/4 + shift)`.
///
/// The first term is the verifiability constraint — the new frame covers at most half the viewport,
/// so the overlap keeps at least half of the *wanted* band. The second term stops the overlap from
/// thinning when the step is large: without it a step of 0.4·extent would leave a 0.1·extent band
/// to measure `gain` and `coverage` on. `max` and not a sum, because only one of the two
/// constraints dominates at a time.
///
/// This is the **wanted** height. The rows a step can actually match are capped by the geometry —
/// `P1.05`'s `match_band` takes `min(wanted, overlap)` — and at `|shift| == extent` there is no
/// overlap at all: that is the `Lost` case (§16.5), not a band-height problem. The band is
/// therefore deliberately **not** clamped to `extent`: clamping it would break the very property
/// the second term exists to provide.
pub(crate) fn band_height(extent: u32, shift: i32) -> u32 {
    let half = extent / 2;
    let scaled = extent as i64 / 4 + shift as i64;
    if scaled <= half as i64 { half } else { scaled as u32 }
}

/// What a step did to the canvas (§17.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepWrite {
    /// Nothing: a duplicate frame (§16.3's `Skip` path). The canvas is byte-identical.
    Skipped,
    /// Nothing: the viewport moved, but it moved **inside** the rows the canvas already has
    /// (§17.4's `Contained`). Not the same as [`Self::Skipped`] — the content moved and the viewport
    /// follows it; there is simply nothing new to write. Treating it as an append would write the
    /// same content a second time, which is the one thing this branch exists to prevent.
    Contained,
    /// `rows` new rows were appended at `first_row` (content coordinates).
    Appended { first_row: u64, rows: u64 },
    /// `rows` new rows were prepended at row 0; every existing row moved down by `rows`.
    Prepended { rows: u64 },
}

/// The scale of a band that holds real canvas pixels. Every band the capture path produces is at
/// this scale; the only other producer of bands is the preview's windowed thumbnail (§19.2).
pub(crate) const FULL_SCALE: u32 = 1;

/// How many bytes one *thumbnail* row of a band at `scale` occupies, or `None` if the scale leaves no
/// room for a row at all.
///
/// A band at scale `s` stores one pixel per `s × s` canvas block, so its rows are `cross_len / s`
/// pixels wide while each one stands for `s` canvas rows. Both facts live here rather than in
/// [`Band::row_count`] so that "one thumbnail row" and "how many canvas rows it covers" cannot drift
/// apart (§19.2: the thumbnail is the same data at a different scale, not a different kind of data).
pub(crate) fn band_row_bytes(cross_len: u64, scale: u32) -> Option<u64> {
    let row_bytes = cross_len / u64::from(scale.max(1)) * BYTES_PER_PIXEL;
    (row_bytes > 0).then_some(row_bytes)
}

/// One whole-width row band (§17.5). `rows` is BGRA in canvas order, `first_row` is its position on
/// the primary axis, and `fnv` is the checksum every load verifies.
///
/// `scale` is what makes a preview band a band: at [`FULL_SCALE`] one stored row is one canvas row,
/// at scale `s` one stored row is `s` canvas rows of `s`-wide blocks (§19.2's windowed thumbnail).
/// [`Self::row_count`] answers in **canvas** rows either way, so every position in the store — the
/// disjointness invariant, the LRU order, `read_rows`, `truncate` — keeps meaning the same thing.
#[derive(Debug, Clone)]
pub(crate) struct Band {
    pub(crate) first_row: u64,
    pub(crate) rows: Vec<u8>,
    pub(crate) fnv: u64,
    pub(crate) scale: u32,
}

impl Band {
    /// A band with its checksum computed. Every producer goes through here rather than filling `fnv`
    /// in by hand: a band that carried a zero checksum would turn the spill verification into a check
    /// that always fails, and one that carried a wrong checksum into a check that always passes —
    /// both worse than having no check at all (§17.5 ⑤, G12).
    pub(crate) fn new(first_row: u64, rows: Vec<u8>) -> Self {
        Self::at_scale(first_row, FULL_SCALE, rows)
    }

    /// A band at an explicit scale: [`Self::new`] for the canvas, and the preview's thumbnail
    /// derivation (§19.2) for everything else.
    pub(crate) fn at_scale(first_row: u64, scale: u32, rows: Vec<u8>) -> Self {
        let fnv = checksum(&rows);
        Self {
            first_row,
            rows,
            fnv,
            scale: scale.max(1),
        }
    }

    /// How many **canvas** rows this band covers, from its byte length, the canvas width and its
    /// scale.
    ///
    /// At `FULL_SCALE` this is the byte length over the row width. At scale `s` the band holds `k`
    /// thumbnail rows of one pixel per `s × s` block, so it stands for `k · s` canvas rows — the same
    /// unit the caller was already using, which is what keeps the fifteen call sites of this function
    /// and of [`Self::end_row`] unchanged.
    pub(crate) fn row_count(&self, cross_len: u64) -> u64 {
        let Some(row_bytes) = band_row_bytes(cross_len, self.scale) else {
            return 0;
        };
        self.rows.len() as u64 / row_bytes * u64::from(self.scale)
    }

    /// The first row *past* this band.
    pub(crate) fn end_row(&self, cross_len: u64) -> u64 {
        self.first_row + self.row_count(cross_len)
    }
}

/// What the band store can fail at (§17.5, §22.3).
///
/// `docs/30` §17.5 names `ErrorCode::CorruptBand`, and this crate has no `ErrorCode`: the session's
/// error enum is §20.4's `StopReason`, which belongs to the session layer (`P3.04`). The store
/// therefore has its own small error type and the session maps it — `MemoryLimit` to
/// `StopReason::MemoryLimit` plus a `Partial` export, `CorruptBand` to "the session fails **without**
/// clearing the canvas" (G12), `Spill` to an internal error. Writing that mapping down here would be
/// guessing at a type that does not exist yet, so it is left to the task that owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BandError {
    /// A band read back from the spill file does not match the checksum it was written with.
    CorruptBand {
        first_row: u64,
        expected: u64,
        found: u64,
    },
    /// The budget is unreachable even with nothing but the protected bands resident (§22.3's third
    /// step: the viewport itself is too large for the budget).
    MemoryLimit { budget: u64, protected: u64 },
    /// The spill file could not be created, written or read.
    Spill { first_row: u64, detail: String },
}

/// The memory the canvas is allowed to keep resident (§22.3).
///
/// All three fields are read, and that is the point: §22.3 notes that the reference implementation
/// carries three budget fields nothing consults, and that V2 does not inherit that. `total` and the
/// two resident counters are read by [`Self::over_budget`]/[`Self::headroom`], and `resident_canvas`
/// is additionally cross-checked against the bytes the store actually holds in
/// [`RecoveredImage::assert_invariants`] — so a bookkeeping drift is an invariant failure rather than
/// a silent overrun.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MemoryBudget {
    total: u64,
    resident_canvas: u64,
    resident_preview: u64,
}

impl MemoryBudget {
    /// §22.3's default: 8 viewports of BGRA. The five consumers §22.3 lists (the reference viewport,
    /// the two bands that cannot be evicted, the band being written, the observation read-back and a
    /// thumbnail preview) come to ≈3.1 viewports, so this leaves 2.5× headroom — and, the part that
    /// matters, it does not depend on how long the content is. A larger default would not buy
    /// correctness: the `GraphicsDevice` is reused across sessions, so the budget is a resident cost.
    pub(crate) const VIEWPORTS: u64 = 8;

    pub(crate) fn for_viewport(cross_len: u64, extent: u64) -> Self {
        Self::with_total(cross_len * extent * BYTES_PER_PIXEL * Self::VIEWPORTS)
    }

    pub(crate) fn with_total(total: u64) -> Self {
        Self {
            total,
            resident_canvas: 0,
            resident_preview: 0,
        }
    }

    pub(crate) fn total(&self) -> u64 {
        self.total
    }

    pub(crate) fn resident_canvas(&self) -> u64 {
        self.resident_canvas
    }

    pub(crate) fn resident_preview(&self) -> u64 {
        self.resident_preview
    }

    /// Everything that counts against `total`. §22.2: the canvas bands and the preview bands share
    /// one budget, and nothing else does — the observation is a step-lifetime buffer.
    pub(crate) fn used(&self) -> u64 {
        self.resident_canvas + self.resident_preview
    }

    pub(crate) fn headroom(&self) -> u64 {
        self.total.saturating_sub(self.used())
    }

    pub(crate) fn over_budget(&self) -> bool {
        self.used() > self.total
    }

    /// Called by the band store, which is the only writer of this number.
    pub(crate) fn set_canvas(&mut self, bytes: u64) {
        self.resident_canvas = bytes;
    }

    /// Called by the preview owner (`P2`): §22.3's first eviction step is the preview's, because a
    /// preview band is rebuildable and a canvas band is not.
    pub(crate) fn set_preview(&mut self, bytes: u64) {
        self.resident_preview = bytes;
    }
}

/// §17.6's first two layers, as values rather than as constants.
///
/// The distinction the layers encode is not "how much" but "what happens when you cross it":
///
/// * **layer 1** (`max_pixels`) is an *architecture* limit. Crossing it stops the canvas at a
///   contiguous prefix and produces a usable `Partial` — never a failure. The default is the `u32`
///   size domain (`u32::MAX / 2` pixels), which is what keeps every `u64` logical interval
///   representable in the final image's dimensions.
/// * **layer 2** (`warn_length`) is a *UI* parameter. Crossing it changes no pixel: it is the point
///   at which the user is told that some viewers cannot open very long images. It is deliberately
///   aligned with PixPin's 29,000 px so that a migrating user sees the prompt in the same place.
///
/// Both are **injectable**, and that is a requirement rather than a convenience: `docs/25` could not
/// locate PixPin's exact total limit, so the number must not be back-inferred from a competitor; our
/// own limit is derived from the size domain and the memory budget, and a test has to be able to
/// lower it (`docs/30` §17.6, property 2).
///
/// Layer 3 — the export budget given by `RowBandSink::begin`'s `estimate_bytes` — is the port's
/// (`P4.05`), because the decision it forces ("trim *before* the first row, never during") can only
/// be expressed where the height is fixed (F-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LongImageLimits {
    max_pixels: u64,
    warn_length: u64,
}

impl LongImageLimits {
    /// `u32::MAX / 2` pixels: the largest total that still leaves every row index inside `u32`.
    pub(crate) const DEFAULT_MAX_PIXELS: u64 = u32::MAX as u64 / 2;
    /// 29,000 px — deliberately the same number PixPin prompts at (§17.6 layer 2).
    pub(crate) const DEFAULT_WARN_LENGTH: u64 = 29_000;

    pub(crate) fn with_max_pixels(max_pixels: u64) -> Self {
        Self {
            max_pixels,
            warn_length: Self::DEFAULT_WARN_LENGTH,
        }
    }

    pub(crate) fn with_warn_length(mut self, warn_length: u64) -> Self {
        self.warn_length = warn_length;
        self
    }

    pub(crate) fn max_pixels(&self) -> u64 {
        self.max_pixels
    }

    pub(crate) fn warn_length(&self) -> u64 {
        self.warn_length
    }

    /// The longest canvas this policy allows, in rows. A pixel budget has to become rows through the
    /// canvas width, which is why this is a method and not a field.
    pub(crate) fn row_limit(&self, cross_len: u64) -> u64 {
        self.max_pixels / cross_len.max(1)
    }

    /// Whether the canvas has reached the architecture limit. The caller trims to [`Self::row_limit`]
    /// and reports `Partial`; it does not fail (§17.6's first non-negotiable property).
    pub(crate) fn reached(&self, primary_len: u64, cross_len: u64) -> bool {
        primary_len >= self.row_limit(cross_len)
    }

    /// Whether the user should be told. Inclusively, and about the *current* length, so that a
    /// session that starts past the threshold still warns once.
    pub(crate) fn warns_at(&self, primary_len: u64) -> bool {
        primary_len >= self.warn_length
    }
}

impl Default for LongImageLimits {
    fn default() -> Self {
        Self::with_max_pixels(Self::DEFAULT_MAX_PIXELS)
    }
}

/// Where a band went on disk, and what it hashed to when it was written (§17.5 ④).
///
/// **There is no `impl Drop for SpillRef` here, and there must not be one.** §22.7's "the file is
/// deleted when its owner drops" reads as if a band owned a file; it does not. One file holds every
/// spilled band (§17.5 ⑥), so a `Drop` on a reference would delete the storage of every *other*
/// band still pointing into it. The owner is [`SpillFile`], the file is released when the last
/// reference leaves ([`BandStore::reclaim_spill_file`]) or when the session ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpillRef {
    pub(crate) first_row: u64,
    pub(crate) row_count: u64,
    pub(crate) offset: u64,
    pub(crate) len: u64,
    pub(crate) fnv: u64,
}

/// The name of the session's spill file inside its directory.
pub(crate) const SPILL_FILE_NAME: &str = "bands.spill";

/// The session's spill file: one file per canvas, appended to in eviction order, removed when it
/// drops. §17.5 ⑥ — the file lives and dies with the session, and nothing is cached across sessions.
///
/// **Rule (`P4.06`, §22.7): the spill file is deleted in `Drop`, never on a shutdown path.** A
/// session can end by panicking, and a cleanup routine that runs after the session is the one thing
/// a panic skips; `Drop` is the only place that runs either way. Grep for `impl Drop for SpillFile`.
struct SpillFile {
    file: std::fs::File,
    path: PathBuf,
    len: u64,
}

impl SpillFile {
    fn create(path: PathBuf) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        Ok(Self { file, path, len: 0 })
    }
}

/// Rule (`P4.06`, §17.5 ⑥): the file is deleted here and nowhere else, so it is deleted on every
/// path — including the one where the session panics. Grep for `impl Drop for SpillFile`.
impl Drop for SpillFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A resident band plus when it was written. The tick is monotonic and lives **inside** the entry so
/// that recency survives [`BandStore::shift_rows`].
struct ResidentBand {
    band: Band,
    written: u64,
}

/// A directory nobody else is using: the process id plus a nanosecond stamp.
fn unique_spill_dir() -> PathBuf {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("snapclip-bands-{}-{token}", std::process::id()))
}

/// The bands of one canvas: whole-width rows, the budget they must fit in, and the spill file for the
/// ones that do not (§17.5, §22.3; `P1.20`).
///
/// Two shapes differ from §17.5's sketch, and each one keeps a fact in a single place:
///
/// * The resident side is a `Vec<ResidentBand>` ordered by `first_row`, not an `LruMap`, and recency
///   is a write tick carried inside the entry. That matters because a prepend (`P1.19`) rewrites
///   every `first_row` through [`Self::shift_rows`]: an LRU keyed by position would have its order
///   scrambled by the canvas growing upwards, while a tick does not move. Position order is what
///   `is_disjoint` and reads need; recency order is what eviction needs. Two orders, kept apart.
/// * Bands are plain `Vec<u8>`, not `Arc<Band>`. Nothing shares a band today — the reference viewport
///   is *copied out* of the store (§17.4) — and an `Arc` nobody clones is a second ownership model
///   sitting next to the store's own.
///
/// The budget is enforced **here** and not by the callers: `insert` records, `relieve` evicts,
/// `read_rows` verifies. `F-07` is a property of the store or it is nothing.
pub(crate) struct BandStore {
    budget: MemoryBudget,
    cross_len: u64,
    resident: Vec<ResidentBand>,
    spilled: BTreeMap<u64, SpillRef>,
    spill: Option<SpillFile>,
    dir: PathBuf,
    owns_dir: bool,
    tick: u64,
}

impl BandStore {
    /// A store that spills into a fresh directory under the system temp directory. The directory —
    /// and with it the spill file — is removed when the store drops.
    pub(crate) fn new(cross_len: u64, budget: MemoryBudget) -> Self {
        Self::in_dir(cross_len, budget, unique_spill_dir())
    }

    /// A store that spills into `dir`. The directory is created if it is missing, and removed again
    /// on drop **only if this store created it**: the tests hand in a directory of their own so that
    /// they can tamper with the spill file (§30.4's corruption case) and still get the cleanup.
    pub(crate) fn in_dir(cross_len: u64, budget: MemoryBudget, dir: PathBuf) -> Self {
        let owns_dir = !dir.exists();
        if owns_dir {
            let _ = std::fs::create_dir_all(&dir);
        }
        Self {
            budget,
            cross_len,
            resident: Vec::new(),
            spilled: BTreeMap::new(),
            spill: None,
            dir,
            owns_dir,
            tick: 0,
        }
    }

    pub(crate) fn budget(&self) -> MemoryBudget {
        self.budget
    }

    /// Adds a band. The band has to be a whole number of rows **at its own scale** for this canvas,
    /// must not overlap what is already there — writing an overlapping band means the caller believes
    /// it knows where the content is and the canvas does not (invariant 6) — and must carry the
    /// checksum of its own bytes, because that checksum is the only thing that makes a later load
    /// verifiable.
    ///
    /// Overlap is checked **within a scale**: a preview band at scale `s` covers exactly the canvas
    /// rows that the full-resolution bands under it cover, and that is the design rather than a
    /// conflict (§19.2 — the thumbnail *is* those rows, smaller).
    ///
    /// `insert` does not evict: [`Self::relieve`] does, once per step, after the caller has said what
    /// the next step needs. Splitting them is what lets one step write a band and still keep the
    /// reference viewport resident for the next one (§17.5 ③).
    pub(crate) fn insert(&mut self, band: Band) {
        let row_bytes = band_row_bytes(self.cross_len, band.scale).unwrap_or(0);
        assert!(
            row_bytes > 0 && band.rows.len() as u64 % row_bytes == 0,
            "a band must be a whole number of rows: {} bytes for a canvas {} px wide at scale {}",
            band.rows.len(),
            self.cross_len,
            band.scale
        );
        assert!(
            self.is_disjoint_from(&band),
            "band {}..{} at scale {} overlaps a band at the same scale that is already in the store",
            band.first_row,
            band.end_row(self.cross_len),
            band.scale
        );
        assert_eq!(
            band.fnv,
            checksum(&band.rows),
            "band {} was stored with a checksum that does not match its own bytes, so a later load would verify nothing",
            band.first_row
        );
        self.tick += 1;
        self.resident.push(ResidentBand {
            band,
            written: self.tick,
        });
        self.resident
            .sort_unstable_by_key(|entry| entry.band.first_row);
        self.sync_accounting();
    }

    /// Evicts resident bands, least recently written first, until the budget is reachable — skipping
    /// every position in `protected`, which is the caller's answer to "what does the next step need"
    /// (§17.5 ③). Returns how many bands left memory.
    ///
    /// Two fates, and §22.3's first eviction step is the order between them: a preview band
    /// (`scale != FULL_SCALE`) is **dropped**, because the thumbnail is derived from the canvas and the
    /// next window refresh rebuilds it; a canvas band is **spilled**, because it is not rebuildable
    /// from anything. So the count is "left memory" and not "went to disk" — `spilled().len()` is the
    /// answer to the narrower question, and `P4.06`'s `spill_file_bytes` is its cost.
    ///
    /// A budget that cannot be reached with nothing but the protected bands resident is
    /// [`BandError::MemoryLimit`] — §22.3's third step, where the viewport itself is too large for the
    /// budget. It is an error and not a silent overrun because an overrun would make the memory bound
    /// hold only while the content is short, which is exactly the property `F-07` exists to provide.
    pub(crate) fn relieve(&mut self, protected: &[u64]) -> Result<u64, BandError> {
        let mut evicted = 0;
        while self.budget.over_budget() {
            let Some(index) = self.least_recently_written(protected) else {
                return Err(BandError::MemoryLimit {
                    budget: self.budget.total(),
                    protected: self.resident_bytes(),
                });
            };
            self.evict(index)?;
            evicted += 1;
        }
        Ok(evicted)
    }

    /// Reads `rows` rows starting at `first_row`, from the resident bands and — for whatever was
    /// evicted — from the spill file, verifying each spilled band's checksum on the way (§17.5 ⑤).
    ///
    /// Takes `&mut self` because a spilled band is fetched with a seek and a read, and because a read
    /// that cannot verify what it got back has to be able to say so. A band read from disk stays on
    /// disk: pulling it back would make residency follow the read pattern instead of the budget.
    pub(crate) fn read_rows(&mut self, first_row: u64, rows: u64) -> Result<Vec<u8>, BandError> {
        let row_bytes = (self.cross_len * BYTES_PER_PIXEL) as usize;
        let end = first_row + rows;
        let mut out = vec![0u8; row_bytes * rows as usize];

        for entry in &self.resident {
            copy_overlap(
                &entry.band.rows,
                entry.band.first_row,
                entry.band.end_row(self.cross_len),
                first_row,
                end,
                row_bytes,
                &mut out,
            );
        }

        let wanted: Vec<SpillRef> = self
            .spilled
            .values()
            .filter(|spilled| spilled.first_row < end && first_row < spilled.first_row + spilled.row_count)
            .copied()
            .collect();
        for spilled in wanted {
            let bytes = self.read_spilled(&spilled)?;
            copy_overlap(
                &bytes,
                spilled.first_row,
                spilled.first_row + spilled.row_count,
                first_row,
                end,
                row_bytes,
                &mut out,
            );
        }
        Ok(out)
    }

    /// The bytes of one spilled band, verified against the checksum it was written with.
    fn read_spilled(&mut self, spilled: &SpillRef) -> Result<Vec<u8>, BandError> {
        use std::io::{Read, Seek, SeekFrom};
        let detail = |error: std::io::Error| BandError::Spill {
            first_row: spilled.first_row,
            detail: error.to_string(),
        };
        let spill = self.spill.as_mut().ok_or_else(|| BandError::Spill {
            first_row: spilled.first_row,
            detail: String::from("a band is recorded as spilled but the session has no spill file"),
        })?;
        spill.file.seek(SeekFrom::Start(spilled.offset)).map_err(detail)?;
        let mut bytes = vec![0u8; spilled.len as usize];
        spill.file.read_exact(&mut bytes).map_err(detail)?;
        let found = checksum(&bytes);
        if found != spilled.fnv {
            return Err(BandError::CorruptBand {
                first_row: spilled.first_row,
                expected: spilled.fnv,
                found,
            });
        }
        Ok(bytes)
    }

    /// The next band to evict: [`BandStore`]'s LRU rule, with §22.3's first step in front of it.
    ///
    /// §22.3 spends the preview's memory first because a preview band is rebuildable and a canvas band
    /// is not, so the search runs twice: once over the preview bands, and only if that finds nothing
    /// over the canvas bands. `protected` is a set of canvas positions and therefore filters only the
    /// second pass — the reference viewport says nothing about a thumbnail, which is dropped and
    /// rebuilt as a whole.
    fn least_recently_written(&self, protected: &[u64]) -> Option<usize> {
        let oldest = |preview: bool| {
            self.resident
                .iter()
                .enumerate()
                .filter(|(_, entry)| (entry.band.scale != FULL_SCALE) == preview)
                .filter(|(_, entry)| preview || !protected.contains(&entry.band.first_row))
                .min_by_key(|(_, entry)| entry.written)
                .map(|(index, _)| index)
        };
        oldest(true).or_else(|| oldest(false))
    }

    /// Drops one band. A preview band is dropped rather than spilled (§22.3's first step): it is
    /// derived from the canvas and the next window refresh rebuilds it, so writing it to disk would
    /// buy a read of something that is already obsolete — and the spill index is keyed by the canvas
    /// position, which the two scales share.
    ///
    /// A canvas band goes to the spill file. One that cannot be written stays resident and the error
    /// comes back to the caller: an eviction that failed silently would be a leak with a checksum.
    fn evict(&mut self, index: usize) -> Result<(), BandError> {
        if self.resident[index].band.scale != FULL_SCALE {
            self.resident.remove(index);
            self.sync_accounting();
            return Ok(());
        }
        let entry = self.resident.remove(index);
        match self.write_spill(&entry.band) {
            Ok(spilled) => {
                self.spilled.insert(entry.band.first_row, spilled);
                self.sync_accounting();
                Ok(())
            }
            Err(error) => {
                self.resident.push(entry);
                self.resident
                    .sort_unstable_by_key(|entry| entry.band.first_row);
                self.sync_accounting();
                Err(error)
            }
        }
    }

    fn write_spill(&mut self, band: &Band) -> Result<SpillRef, BandError> {
        use std::io::{Seek, SeekFrom, Write};
        let first_row = band.first_row;
        let detail = |error: std::io::Error| BandError::Spill {
            first_row,
            detail: error.to_string(),
        };
        if self.spill.is_none() {
            let path = self.dir.join(SPILL_FILE_NAME);
            self.spill = Some(SpillFile::create(path).map_err(detail)?);
        }
        let spill = self
            .spill
            .as_mut()
            .expect("the spill file was just created if it was missing");
        let offset = spill.len;
        spill.file.seek(SeekFrom::Start(offset)).map_err(detail)?;
        spill.file.write_all(&band.rows).map_err(detail)?;
        spill.file.flush().map_err(detail)?;
        spill.len = offset + band.rows.len() as u64;
        Ok(SpillRef {
            first_row,
            row_count: band.row_count(self.cross_len),
            offset,
            len: band.rows.len() as u64,
            fnv: band.fnv,
        })
    }

    /// The store's own bookkeeping, written into the budget after every change so that
    /// [`MemoryBudget::resident_canvas`] and [`MemoryBudget::resident_preview`] always mean "what this
    /// store holds".
    ///
    /// Two setters and not one sum, because §22.3's eviction order is a decision about which half is
    /// which: the preview half is spent first, so the budget has to know how much of it there is. A
    /// single number would make that step unrepresentable.
    fn sync_accounting(&mut self) {
        self.budget.set_canvas(self.resident_canvas_bytes());
        self.budget.set_preview(self.resident_preview_bytes());
    }

    /// The size of the session's spill file in bytes, or `0` if nothing has been evicted yet.
    ///
    /// `E-MEM-1` (§23.1) records this **separately** from `allocated`/`peak`, and §6's benchmark
    /// discipline says why: *heap traffic cannot prove a space reduction when the storage moved to an
    /// OS mapping*. A memory number that went down while these bytes went up is not a smaller
    /// footprint, it is a footprint in a different place — so the bytes that moved to disk get their
    /// own column instead of being inferred from the heap.
    pub(crate) fn spill_file_bytes(&self) -> u64 {
        self.spill.as_ref().map_or(0, |spill| spill.len)
    }

    /// Drops the spill file once nothing points into it.
    ///
    /// The file is one file for the whole session (§17.5 ⑥), so it cannot follow an individual band
    /// out — but it must not outlive the **last** band that points into it either: from that moment
    /// the bytes on disk are a cost with no reader, and §22.7's "no residue" is a claim about the
    /// session, not about its happy path. `write_spill` recreates the file on demand, so the store
    /// loses nothing by letting go early.
    fn reclaim_spill_file(&mut self) {
        if self.spilled.is_empty() {
            self.spill = None;
        }
    }

    /// The rows the canvas proper holds — every band at [`FULL_SCALE`].
    ///
    /// Invariant 7's canvas half compares this with [`MemoryBudget::resident_canvas`]; the sum of the
    /// two halves is [`Self::resident_bytes`].
    pub(crate) fn resident_canvas_bytes(&self) -> u64 {
        self.resident
            .iter()
            .filter(|entry| entry.band.scale == FULL_SCALE)
            .map(|entry| entry.band.rows.len() as u64)
            .sum()
    }

    /// The rows the preview holds — every band that is not at [`FULL_SCALE`], which today means the
    /// windowed thumbnail of §19.2.
    ///
    /// This is the number `E-MEM-1`'s "thumbnail memory does not follow the content length" claim is
    /// read off, and the number §22.3's first eviction step spends.
    pub(crate) fn resident_preview_bytes(&self) -> u64 {
        self.resident
            .iter()
            .filter(|entry| entry.band.scale != FULL_SCALE)
            .map(|entry| entry.band.rows.len() as u64)
            .sum()
    }

    /// Throws away every band that is not canvas content.
    ///
    /// The thumbnail is a function of the canvas **and of the canvas' origin**: its rows are aligned
    /// to multiples of the scale, so a prepend, a truncation or a removed prefix can leave one that
    /// starts mid-block. Rather than carry half-aligned bands, the store drops them — they are
    /// rebuildable from the canvas, which is exactly the property that made them the first thing to
    /// evict. Returns how many bands went.
    pub(crate) fn drop_previews(&mut self) -> u64 {
        let before = self.resident.len();
        self.resident
            .retain(|entry| entry.band.scale == FULL_SCALE);
        let dropped = (before - self.resident.len()) as u64;
        if dropped > 0 {
            self.sync_accounting();
        }
        dropped
    }

    fn is_disjoint_from(&self, candidate: &Band) -> bool {
        self.resident
            .iter()
            .filter(|entry| entry.band.scale == candidate.scale)
            .all(|entry| {
                candidate.first_row >= entry.band.end_row(self.cross_len)
                    || entry.band.first_row >= candidate.end_row(self.cross_len)
            })
    }

    /// Invariant 6: no two bands **at the same scale** share a row. Bands at different scales overlap
    /// by construction — a thumbnail covers the canvas rows it was made from — so the comparison is
    /// per scale and the invariant is still about each position having one owner.
    pub(crate) fn is_disjoint(&self) -> bool {
        let mut scales: Vec<u32> = self
            .resident
            .iter()
            .map(|entry| entry.band.scale)
            .collect();
        scales.sort_unstable();
        scales.dedup();
        scales.into_iter().all(|scale| {
            let mut ranges: Vec<(u64, u64)> = self
                .resident
                .iter()
                .filter(|entry| entry.band.scale == scale)
                .map(|entry| (entry.band.first_row, entry.band.end_row(self.cross_len)))
                .collect();
            ranges.sort_unstable();
            ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0)
        })
    }

    /// Moves every band down by `rows`. Used by a prepend (`P1.19`): the content did not move, the
    /// canvas grew **above** it, so the coordinates of everything already stored shift by the number
    /// of rows that were inserted in front of them — on disk as well as in memory, or a spilled band
    /// would come back at the wrong place after a prepend.
    pub(crate) fn shift_rows(&mut self, rows: u64) {
        if rows == 0 {
            return;
        }
        self.drop_previews();
        for entry in &mut self.resident {
            entry.band.first_row += rows;
        }
        self.resident
            .sort_unstable_by_key(|entry| entry.band.first_row);
        let moved: Vec<SpillRef> = std::mem::take(&mut self.spilled)
            .into_values()
            .map(|mut spilled| {
                spilled.first_row += rows;
                spilled
            })
            .collect();
        for spilled in moved {
            self.spilled.insert(spilled.first_row, spilled);
        }
    }

    /// Moves every band back up by `rows` — the inverse of [`Self::shift_rows`], used when a prepend
    /// is undone (`P1.22`). Like the forward direction it has to move the spilled keys too, or a band
    /// that was evicted before the undo would come back at the wrong place.
    pub(crate) fn unshift_rows(&mut self, rows: u64) {
        if rows == 0 {
            return;
        }
        self.drop_previews();
        for entry in &mut self.resident {
            entry.band.first_row = entry
                .band
                .first_row
                .checked_sub(rows)
                .expect("a band that starts before the rows being removed cannot be shifted back");
        }
        self.resident
            .sort_unstable_by_key(|entry| entry.band.first_row);
        let moved: Vec<SpillRef> = std::mem::take(&mut self.spilled)
            .into_values()
            .map(|mut spilled| {
                spilled.first_row = spilled
                    .first_row
                    .checked_sub(rows)
                    .expect("a spilled band that starts before the rows being removed cannot be shifted back");
                spilled
            })
            .collect();
        for spilled in moved {
            self.spilled.insert(spilled.first_row, spilled);
        }
    }

    /// Drops the band that covers `[0, rows)`. A prepend always inserts exactly one such band
    /// (`P1.19`), and undoing it removes exactly that band — resident or spilled, because where a band
    /// happens to live is not part of what a band *is*.
    pub(crate) fn remove_leading(&mut self, rows: u64) {
        self.drop_previews();
        if let Some(first) = self.resident.first() {
            if first.band.first_row == 0 {
                assert_eq!(
                    first.band.row_count(self.cross_len),
                    rows,
                    "the leading band is {} rows, not the {rows} rows this undo removes",
                    first.band.row_count(self.cross_len)
                );
                self.resident.remove(0);
                self.sync_accounting();
                return;
            }
        }
        if let Some(spilled) = self.spilled.remove(&0) {
            assert_eq!(
                spilled.row_count, rows,
                "the leading spilled band is {} rows, not the {rows} rows this undo removes",
                spilled.row_count
            );
            self.reclaim_spill_file();
            self.sync_accounting();
            return;
        }
        panic!("no band starts at row 0: a prepend inserts one, and only an undo of that prepend removes it");
    }

    /// Invariant 7's left-hand side: what is resident **now**, both halves together.
    ///
    /// The halves have their own readers ([`Self::resident_canvas_bytes`],
    /// [`Self::resident_preview_bytes`]) because they answer different questions; this is the total,
    /// which is what `relieve` compares against and what its `MemoryLimit` reports.
    pub(crate) fn resident_bytes(&self) -> u64 {
        self.resident
            .iter()
            .map(|entry| entry.band.rows.len() as u64)
            .sum()
    }

    /// Keeps only the bands that cover rows `[0, rows)` — §17.6 layer 1's "contiguous prefix",
    /// performed on the store. A band that straddles `rows` is **truncated**, not dropped: its first
    /// `rows − first_row` rows belong to the prefix, and dropping them would leave a hole.
    ///
    /// A straddling *spilled* band is read back, truncated and re-inserted as resident, because a
    /// `SpillRef`'s checksum covers exactly the bytes it points at (§17.5 ⑤): narrowing the reference
    /// in place would make every later load fail the check. Re-reading is bounded (at most one band,
    /// once per session) and the band may be spilled again by the next `relieve`.
    ///
    /// This is the only operation in this module that destroys content, and it is deliberately the
    /// only one that cannot fail silently: every read goes through the checksum.
    pub(crate) fn truncate(&mut self, rows: u64) -> Result<(), BandError> {
        self.drop_previews();
        let row_bytes = (self.cross_len * BYTES_PER_PIXEL) as usize;
        let mut kept: Vec<ResidentBand> = Vec::with_capacity(self.resident.len());
        for entry in self.resident.drain(..) {
            let first_row = entry.band.first_row;
            if first_row >= rows {
                continue;
            }
            if entry.band.end_row(self.cross_len) <= rows {
                kept.push(entry);
                continue;
            }
            let keep_rows = (rows - first_row) as usize;
            let bytes = entry.band.rows[..keep_rows * row_bytes].to_vec();
            kept.push(ResidentBand {
                band: Band::new(first_row, bytes),
                written: entry.written,
            });
        }
        self.resident = kept;

        let straddling: Vec<u64> = self
            .spilled
            .iter()
            .filter(|(first_row, spilled)| {
                **first_row < rows && **first_row + spilled.row_count > rows
            })
            .map(|(first_row, _)| *first_row)
            .collect();
        for first_row in straddling {
            let spilled = *self
                .spilled
                .get(&first_row)
                .expect("collected from this map a moment ago");
            let bytes = self.read_spilled(&spilled)?;
            self.spilled.remove(&first_row);
            let keep_rows = (rows - first_row) as usize;
            self.insert(Band::new(first_row, bytes[..keep_rows * row_bytes].to_vec()));
        }
        self.spilled.retain(|first_row, _| *first_row < rows);
        self.reclaim_spill_file();

        self.sync_accounting();
        Ok(())
    }

    pub(crate) fn is_resident(&self, first_row: u64) -> bool {
        self.resident
            .iter()
            .any(|entry| entry.band.first_row == first_row)
    }

    pub(crate) fn spilled(&self) -> &BTreeMap<u64, SpillRef> {
        &self.spilled
    }

    /// The resident bands, in position order.
    pub(crate) fn bands(&self) -> impl Iterator<Item = &Band> {
        self.resident.iter().map(|entry| &entry.band)
    }

    /// The `count` most recently written bands, newest last. §17.5 ③ protects them from eviction:
    /// they are what a small rollback reads (§17.4's `Contained`).
    pub(crate) fn most_recent(&self, count: usize) -> Vec<&Band> {
        let mut entries: Vec<&ResidentBand> = self
            .resident
            .iter()
            .filter(|entry| entry.band.scale == FULL_SCALE)
            .collect();
        entries.sort_unstable_by_key(|entry| entry.written);
        entries
            .into_iter()
            .rev()
            .take(count)
            .map(|entry| &entry.band)
            .collect()
    }

    /// The canvas bands, without the preview's. [`Self::bands`] is every resident band — the two
    /// answers differ because a thumbnail covers canvas rows it is not a copy of, so anything asking
    /// "which rows does the canvas hold" has to exclude it.
    pub(crate) fn canvas_bands(&self) -> impl Iterator<Item = &Band> {
        self.resident
            .iter()
            .filter(|entry| entry.band.scale == FULL_SCALE)
            .map(|entry| &entry.band)
    }

    /// Test-only: the budget is the store's, and the invariant cases have to build a store that is
    /// already over it (invariant 7) or already inconsistent (invariant 6).
    #[cfg(test)]
    pub(crate) fn set_budget(&mut self, budget: MemoryBudget) {
        self.budget = budget;
        self.sync_accounting();
    }

    /// Test-only: install resident bands directly, bypassing the checks `insert` makes. Invariant 6's
    /// case is two bands that overlap, which the public API refuses to build — deliberately, since
    /// that is what the invariant is there to catch.
    #[cfg(test)]
    pub(crate) fn set_resident(&mut self, bands: Vec<Band>) {
        self.tick = 0;
        self.resident = bands
            .into_iter()
            .map(|band| {
                self.tick += 1;
                ResidentBand {
                    band,
                    written: self.tick,
                }
            })
            .collect();
        self.resident
            .sort_unstable_by_key(|entry| entry.band.first_row);
        self.sync_accounting();
    }
}

/// Rule (`P4.06`, §22.7): the spill file is removed in `Drop`, and the directory goes with it when
/// this store is the one that created it. Grep for `impl Drop for BandStore`.
impl Drop for BandStore {
    fn drop(&mut self) {
        // The spill file first (its own `Drop`), then the directory — but only if we made it.
        self.spill = None;
        if self.owns_dir {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Copies the rows a stored band shares with `[first_row, end)` into `out`, which is laid out for
/// that same range. One helper for the resident and the spilled path, so a band cannot be read one
/// way from memory and another way from disk.
fn copy_overlap(
    band: &[u8],
    band_first: u64,
    band_end: u64,
    first_row: u64,
    end: u64,
    row_bytes: usize,
    out: &mut [u8],
) {
    let start = band_first.max(first_row);
    let stop = band_end.min(end);
    if start >= stop {
        return;
    }
    let src = (start - band_first) as usize * row_bytes;
    let dst = (start - first_row) as usize * row_bytes;
    let len = (stop - start) as usize * row_bytes;
    out[dst..dst + len].copy_from_slice(&band[src..src + len]);
}

/// Which rows of the canvas have been written at least once (§17.1's coverage invariant).
///
/// One bit per row: a 100,000-row canvas is 12.5 KiB here, and **no pixels live in this type** — the
/// "canvas bitmap" of V1 does not exist. See the module doc for why the interval ends are derived.
pub(crate) struct CoverageMap {
    covered: Vec<u64>,
    first: Option<u64>,
    last_exclusive: u64,
    rows_covered: u64,
    pub(crate) stale_steps: u32,
}

impl CoverageMap {
    pub(crate) fn new(primary_len: u64) -> Self {
        Self {
            covered: vec![0; words_for(primary_len)],
            first: None,
            last_exclusive: 0,
            rows_covered: 0,
            stale_steps: 0,
        }
    }

    /// Marks `[start, end)` as written and returns how many rows were newly covered. Growing the
    /// bitmap is this method's business, so a caller cannot forget to size it.
    pub(crate) fn mark_range(&mut self, start: u64, end: u64) -> u64 {
        if start >= end {
            return 0;
        }
        self.ensure_rows(end);

        let mut added = 0;
        let mut row = start;
        while row < end {
            let word = (row / 64) as usize;
            let bit = row % 64;
            if bit == 0 && row + 64 <= end {
                added += 64 - self.covered[word].count_ones() as u64;
                self.covered[word] = u64::MAX;
                row += 64;
            } else {
                let mask = 1u64 << bit;
                if self.covered[word] & mask == 0 {
                    self.covered[word] |= mask;
                    added += 1;
                }
                row += 1;
            }
        }

        if added > 0 {
            self.first = Some(self.first.map_or(start, |first| first.min(start)));
            self.last_exclusive = self.last_exclusive.max(end);
            self.rows_covered += added;
        }
        added
    }

    pub(crate) fn is_covered(&self, row: u64) -> bool {
        let word = (row / 64) as usize;
        word < self.covered.len() && (self.covered[word] >> (row % 64)) & 1 == 1
    }

    /// Invariant 2's left-hand side: the lowest covered row, or 0 when nothing is covered.
    pub(crate) fn span_start(&self) -> u64 {
        self.first.unwrap_or(0)
    }

    /// Invariant 3's left-hand side: one past the highest covered row.
    pub(crate) fn span_end(&self) -> u64 {
        self.last_exclusive
    }

    pub(crate) fn rows_covered(&self) -> u64 {
        self.rows_covered
    }

    /// Invariant 4: the number of covered rows equals the width of the covered interval, so there is
    /// no gap inside it. O(1) — the count is maintained by [`Self::mark_range`].
    pub(crate) fn has_hole(&self) -> bool {
        self.rows_covered != self.span_end() - self.span_start()
    }

    fn ensure_rows(&mut self, rows: u64) {
        let words = words_for(rows);
        if self.covered.len() < words {
            self.covered.resize(words, 0);
        }
    }

    /// Makes room for `rows` rows **in front** of the ones already marked, shifting every existing
    /// bit up by `rows` and marking the new front as covered. A prepend (`P1.19`) is the only caller.
    ///
    /// The shift is a real bit shift over the whole map rather than a stored offset, because the
    /// coverage interval is anchored at 0 (invariant 2) and a bias field would put the anchor in two
    /// places at once. `last_exclusive` moves by `rows` and then [`Self::mark_range`] fills the front:
    /// the count of covered rows has to keep answering invariant 4 in O(1), so the shifted bits are
    /// never recounted — only the newly written front rows are.
    pub(crate) fn insert_rows_at_front(&mut self, rows: u64) {
        if rows == 0 {
            return;
        }
        let shift_words = (rows / 64) as usize;
        let shift_bits = (rows % 64) as u32;
        let old_words = self.covered.len();
        let new_words = old_words + words_for(rows);
        self.covered.resize(new_words, 0);
        // From the top down, so every source word is read before it is overwritten.
        for index in (0..new_words).rev() {
            let high = if index >= shift_words {
                self.covered[index - shift_words]
            } else {
                0
            };
            let low = if index > shift_words {
                self.covered[index - shift_words - 1]
            } else {
                0
            };
            self.covered[index] = if shift_bits == 0 {
                high
            } else {
                (high << shift_bits) | (low >> (64 - shift_bits))
            };
        }
        self.last_exclusive += rows;
        self.mark_range(0, rows);
    }

    /// The inverse of [`Self::insert_rows_at_front`]: drops the first `rows` rows and shifts every
    /// remaining bit down. Undoing a prepend (`P1.22`) is the only caller.
    ///
    /// Both counters are **recounted** rather than decremented, for the same reason [`Self::truncate`]
    /// recounts: this runs once per user undo, not once per step, and a recount is the version that
    /// cannot drift. A prepend marks its front covered, so the bits being dropped here are always set
    /// — but the general shift is written out anyway, because "the caller only ever asks for covered
    /// rows" is exactly the kind of assumption that stops being true quietly.
    pub(crate) fn remove_rows_at_front(&mut self, rows: u64) {
        if rows == 0 {
            return;
        }
        let shift_words = (rows / 64) as usize;
        let shift_bits = (rows % 64) as u32;
        let old_words = self.covered.len();
        let new_words = old_words.saturating_sub(words_for(rows));
        // From the bottom up, so every source word is read before it is overwritten.
        for index in 0..new_words {
            let high = if index + shift_words < old_words {
                self.covered[index + shift_words]
            } else {
                0
            };
            let low = if index + shift_words + 1 < old_words {
                self.covered[index + shift_words + 1]
            } else {
                0
            };
            self.covered[index] = if shift_bits == 0 {
                high
            } else {
                (high >> shift_bits) | (low << (64 - shift_bits))
            };
        }
        self.covered.truncate(new_words);
        self.rows_covered = self.covered.iter().map(|word| word.count_ones() as u64).sum();
        self.last_exclusive = self.last_exclusive.saturating_sub(rows);
        self.first = if self.rows_covered == 0 {
            None
        } else {
            self.covered
                .iter()
                .position(|word| *word != 0)
                .map(|index| index as u64 * 64 + self.covered[index].trailing_zeros() as u64)
        };
    }

    /// Keeps only the first `rows` rows — §17.6 layer 1's "contiguous prefix". The dropped tail is
    /// gone rather than hidden, so a hole can never appear: whatever the caller does with the
    /// remaining rows, invariant 4 still answers `rows_covered == span_end − span_start`.
    ///
    /// The count is recounted here instead of decremented. This runs **once** per session (at the
    /// architecture limit) and a recount is the version that cannot drift; the O(1) rule belongs to
    /// [`Self::mark_range`], which runs every step.
    pub(crate) fn truncate(&mut self, rows: u64) {
        let words = words_for(rows);
        self.covered.truncate(words);
        if let Some(last) = self.covered.last_mut() {
            let keep = rows % 64;
            if keep != 0 {
                *last &= (1u64 << keep) - 1;
            }
        }
        self.rows_covered = self.covered.iter().map(|word| word.count_ones() as u64).sum();
        self.last_exclusive = self.last_exclusive.min(rows);
        self.first = if self.rows_covered == 0 {
            None
        } else {
            self.covered
                .iter()
                .position(|word| *word != 0)
                .map(|index| index as u64 * 64 + self.covered[index].trailing_zeros() as u64)
        };
    }
}

fn words_for(rows: u64) -> usize {
    rows.div_ceil(64) as usize
}

/// What one step did with the frame it was given. §20.3 invariant 5 is the statement that these
/// three numbers add up, and it is a *type* rather than three loose arguments so that a caller
/// cannot pass two of them by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StepTally {
    pub(crate) step: u64,
    pub(crate) committed: u64,
    pub(crate) discarded: u64,
}

/// The recovered image: a logical interval on the primary axis, the coverage of it, and the bands.
///
/// There is no `Vec<u8>` of the whole image anywhere in this type, which is the point (§17.1): a
/// 100,000-row canvas at 1500 px is 600 MB, and `docs/30` §17.5 requires memory to be independent of
/// the image's length.
pub(crate) struct RecoveredImage {
    pub(crate) axis: Axis,
    pub(crate) primary_len: u64,
    pub(crate) cross_len: u64,
    pub(crate) coverage: CoverageMap,
    pub(crate) bands: BandStore,
    owner: std::thread::ThreadId,
}

impl RecoveredImage {
    /// A canvas whose bands spill into a fresh directory of their own.
    pub(crate) fn new(axis: Axis, cross_len: u64, budget: MemoryBudget) -> Self {
        Self::in_dir(axis, cross_len, budget, unique_spill_dir())
    }

    /// A canvas whose bands spill into `dir` (§17.5 ④: the session's own temporary file).
    pub(crate) fn in_dir(
        axis: Axis,
        cross_len: u64,
        budget: MemoryBudget,
        dir: PathBuf,
    ) -> Self {
        Self {
            axis,
            primary_len: 0,
            cross_len,
            coverage: CoverageMap::new(0),
            bands: BandStore::in_dir(cross_len, budget, dir),
            owner: std::thread::current().id(),
        }
    }

    pub(crate) fn axis(&self) -> Axis {
        self.axis
    }

    pub(crate) fn primary_len(&self) -> u64 {
        self.primary_len
    }

    pub(crate) fn cross_len(&self) -> u64 {
        self.cross_len
    }

    pub(crate) fn coverage(&self) -> &CoverageMap {
        &self.coverage
    }

    pub(crate) fn bands(&self) -> &BandStore {
        &self.bands
    }

    /// The store, mutably: the preview's thumbnail derivation (§19.2) puts its band in through here.
    ///
    /// It is the same store the capture path writes to, which is the whole point of ADR-9 — the
    /// thumbnail shares the budget, the LRU and the eviction order instead of bringing its own. A
    /// second store would need a second budget, and two budgets cannot be spent in the order §22.3
    /// describes.
    pub(crate) fn bands_mut(&mut self) -> &mut BandStore {
        &mut self.bands
    }

    /// The first frame establishes the canvas: every row it shows is new content.
    ///
    /// This is the only call that writes a whole viewport at once, and §17.3's rule is vacuous here
    /// because there is nothing to overwrite yet.
    pub(crate) fn start(&mut self, frame: &Observation) {
        assert_eq!(
            self.primary_len, 0,
            "the canvas already has content: only the first frame creates it"
        );
        assert_eq!(
            frame.width() as u64,
            self.cross_len,
            "the frame's cross axis is not the canvas's — the canvas width is fixed for the session (invariant 1)"
        );
        let rows = frame.height() as u64;
        let row_bytes = (self.cross_len * BYTES_PER_PIXEL) as usize;
        assert_eq!(
            frame.pixels().len(),
            row_bytes * rows as usize,
            "the frame is not strictly packed: the canvas stores whole rows"
        );
        self.bands
            .insert(Band::new(0, frame.pixels().to_vec()));
        self.primary_len = rows;
        self.coverage = CoverageMap::new(rows);
        self.coverage.mark_range(0, rows);
    }

    /// Writes the rows one confirmed step adds below the canvas (§17.3): `[old end, old end + rows)`,
    /// taken from the **bottom** of the frame — the frame's viewport ends at the new content end, so
    /// the new content is the frame's last `rows` rows.
    ///
    /// The overlap is never rewritten. Three reasons, and none of them is "it is safer" (§17.3):
    /// the overlap is not the same thing in the two frames (a `fixed` header or an animation would
    /// be stamped into the canvas on every step, which is the one place where the region model and
    /// the write rule would disagree), a rewrite is how drift gets *into* the canvas (the residual
    /// gate is a frame-pair judgement, so nothing at the canvas level would stop "every step right,
    /// whole canvas wrong"), and first-writer-wins makes every row answer "which step produced me"
    /// — which is what `Review` and the undo of §19.6 are built on.
    ///
    /// The cost is written down too: a ±1 px error that passes all four gates is *frozen* into the
    /// seam instead of being repainted by a later frame. That trade is deliberate — rare and local
    /// (four gates plus the prior) against common and cumulative (§17.4's reference design is what
    /// keeps it from propagating).
    ///
    /// `rows` is how many rows are genuinely new. In the plain append it is the step's displacement
    /// `d`; a viewport that jumped further than the canvas' end adds only what fits, and
    /// [`ViewportState::apply`] is what computes that number. The direction is not this method's
    /// business: a prepend is [`Self::prepend_confirmed`], and the caller decides which one a step
    /// is (`ViewportState`).
    pub(crate) fn append_confirmed(&mut self, frame: &Observation, rows: u64) -> StepWrite {
        if rows == 0 {
            return StepWrite::Skipped;
        }
        let extent = frame.height() as u64;
        assert_eq!(
            frame.width() as u64,
            self.cross_len,
            "the frame's cross axis is not the canvas's (invariant 1)"
        );
        assert!(
            rows <= extent,
            "a step that adds {rows} rows cannot come out of a viewport of {extent} rows"
        );
        assert!(
            rows <= band_height(extent as u32, rows as i32) as u64,
            "the band ({}) does not contain the {rows} rows this step adds: §17.2's first term is what makes the write possible at all",
            band_height(extent as u32, rows as i32)
        );
        let first_row = self.primary_len;
        let row_bytes = (self.cross_len * BYTES_PER_PIXEL) as usize;
        let first_frame_row = (extent - rows) as usize * row_bytes;
        let new_rows = &frame.pixels()[first_frame_row..];
        assert_eq!(
            new_rows.len(),
            row_bytes * rows as usize,
            "the frame is not strictly packed: the canvas stores whole rows"
        );
        self.bands.insert(Band::new(first_row, new_rows.to_vec()));
        self.primary_len += rows;
        self.coverage.mark_range(first_row, self.primary_len);
        StepWrite::Appended { first_row, rows }
    }

    /// Writes the rows a confirmed step adds **above** the canvas (§17.1's "both directions"): the
    /// frame's first `rows` rows, at content row 0, with every existing row moved down by `rows`.
    ///
    /// This is what makes §17.4's "the reference is always the canvas" affordable in both directions
    /// and what `F-02` is about: `next_pos = current_pos + signed_delta` — a negative displacement is
    /// a fact about the page, not an error. `Contained` is the other side of the same coin and is
    /// decided before this is called ([`ViewportState::apply`]).
    ///
    /// The bound is gate one's (`|d| <= extent`, §16.2), read in this direction, and the acceptance
    /// scan is what settled it (`P1.24`): the first version of this assertion said `extent / 2`, and
    /// the `|d| = 500` rows of the scan then tripped it on a step the whole funnel had just
    /// confirmed with all four gates. The estimator is right to confirm it — 500 of 900 rows still
    /// leaves 400 rows of overlap, which is most of a match band — and refusing to write would leave
    /// the canvas stale after a legitimate scroll-up. §17.2's band formula is satisfied for every
    /// `rows <= extent` in this direction: the band is the frame's *leading* rows, so
    /// `max(extent / 2, extent / 4 + rows) >= rows` holds trivially.
    ///
    /// It stays an assertion rather than a silent trim because a caller that hands us more than the
    /// viewport has skipped the gate that says so.
    pub(crate) fn prepend_confirmed(&mut self, frame: &Observation, rows: u64) -> StepWrite {
        if rows == 0 {
            return StepWrite::Contained;
        }
        let extent = frame.height() as u64;
        assert_eq!(
            frame.width() as u64,
            self.cross_len,
            "the frame's cross axis is not the canvas's (invariant 1)"
        );
        assert!(
            rows <= extent,
            "a prepend of {rows} rows is more than the {extent}-row viewport it came from, so it is \
             not a displacement gate one would have passed (§16.2)"
        );
        let row_bytes = (self.cross_len * BYTES_PER_PIXEL) as usize;
        let new_rows = &frame.pixels()[..rows as usize * row_bytes];
        assert_eq!(
            new_rows.len(),
            row_bytes * rows as usize,
            "the frame is not strictly packed: the canvas stores whole rows"
        );
        self.bands.shift_rows(rows);
        self.bands.insert(Band::new(0, new_rows.to_vec()));
        self.primary_len += rows;
        self.coverage.insert_rows_at_front(rows);
        StepWrite::Prepended { rows }
    }

    /// The largest position the viewport's top edge can take without leaving the canvas.
    pub(crate) fn max_position(&self, extent: u64) -> i64 {
        self.primary_len as i64 - extent as i64
    }

    /// A copy of `rows` primary-axis rows starting at `first_row`, reassembled across the bands.
    ///
    /// There is no single buffer to hand out (§17.1), so a reader asks for the range it needs: the
    /// export sink (`P4.01`), the preview and the tests all read through here. Rows the canvas does
    /// not have are a caller error, not a zero-filled range.
    ///
    /// This is fallible since `P1.20`: a band that is no longer resident has to be fetched from the
    /// spill file, and a fetch that cannot verify its checksum is a failure the caller has to see
    /// (§17.5 ⑤, G12) rather than a silently zero-filled range.
    pub(crate) fn rows(&mut self, first_row: u64, rows: u64) -> Result<Vec<u8>, BandError> {
        assert!(
            first_row + rows <= self.primary_len,
            "asked for rows {first_row}..{} but the canvas ends at {}",
            first_row + rows,
            self.primary_len
        );
        self.bands.read_rows(first_row, rows)
    }

    /// Evicts what the next step does not need (§17.5 ③, §22.3). `reference` is the viewport the
    /// caller is about to use — `(position, extent)` — and it is protected along with the two most
    /// recently written bands, because those are what a small rollback reads.
    ///
    /// The set is computed **here** and not inside the store: the store knows about bands, the canvas
    /// knows which rows the next step will ask for, and §17.5 ③ is a statement about the latter.
    pub(crate) fn relieve(&mut self, reference: Option<(i64, u64)>) -> Result<u64, BandError> {
        let protected = self.protected_rows(reference);
        self.bands.relieve(&protected)
    }

    /// §17.6 layer 1's stop: keep the first `rows` rows and drop the tail, so that what remains is a
    /// **contiguous prefix** — a usable `Partial`, not a failure. Returns how many rows were dropped.
    ///
    /// `rows == 0` is refused rather than served: an image with no rows is not a partial anything, and
    /// the case only arises when the viewport alone exceeds the pixel budget — which is a statement
    /// about the viewport, not about the content, and belongs to the caller (§20.4).
    pub(crate) fn trim_to(&mut self, rows: u64) -> Result<u64, BandError> {
        assert!(
            rows > 0,
            "a canvas trimmed to zero rows is not a Partial: a budget smaller than one row means the viewport does not fit"
        );
        assert!(
            rows <= self.primary_len,
            "trim_to({rows}) would extend a {}-row canvas: trimming only ever removes the tail",
            self.primary_len
        );
        let dropped = self.primary_len - rows;
        if dropped == 0 {
            return Ok(0);
        }
        self.bands.truncate(rows)?;
        self.primary_len = rows;
        self.coverage.truncate(rows);
        Ok(dropped)
    }

    /// The other direction of [`Self::trim_to`]: drops the first `rows` rows and shifts the rest up,
    /// for undoing a prepend (`P1.22`). Returns the rows removed.
    ///
    /// `trim_to` cannot serve here even though both shrink the canvas: a prepend adds rows at the
    /// **front**, so the rows an undo has to delete are the front ones, and trimming the tail would
    /// keep the wrong content. Because the undo history is LIFO, a prepend that could not be undone
    /// would block every earlier undo behind it.
    ///
    /// The failure type is `BandError` only so that `undo_last` has a single error type; nothing here
    /// can fail today.
    pub(crate) fn remove_prefix(&mut self, rows: u64) -> Result<u64, BandError> {
        assert!(
            rows > 0,
            "removing zero rows is not an undo; the caller's mark says how many the prepend added"
        );
        assert!(
            rows < self.primary_len,
            "removing {rows} of {} rows would leave no canvas at all",
            self.primary_len
        );
        self.bands.remove_leading(rows);
        self.bands.unshift_rows(rows);
        self.primary_len -= rows;
        self.coverage.remove_rows_at_front(rows);
        Ok(rows)
    }

    fn protected_rows(&self, reference: Option<(i64, u64)>) -> Vec<u64> {
        let mut protected = Vec::new();
        if let Some((position, extent)) = reference {
            let start = position.max(0) as u64;
            let end = start + extent;
            for band in self.bands.canvas_bands() {
                if band.first_row < end && start < band.end_row(self.cross_len) {
                    protected.push(band.first_row);
                }
            }
        }
        protected.extend(
            self.bands
                .most_recent(2)
                .into_iter()
                .map(|band| band.first_row),
        );
        protected.sort_unstable();
        protected.dedup();
        protected
    }

    /// §20.3's eight invariants, checked after every step. The messages name the invariant by number
    /// so that a failure says *which* promise broke, and the test that constructs each violation
    /// checks exactly that.
    pub(crate) fn assert_invariants(&self, viewport_cross: u64, tally: StepTally) {
        assert_eq!(
            self.cross_len, viewport_cross,
            "invariant 1 (docs/30 §20.3): the cross axis is constant for the whole session — canvas {} px, viewport {} px",
            self.cross_len, viewport_cross
        );
        assert_eq!(
            self.coverage.span_start(),
            0,
            "invariant 2 (docs/30 §20.3): coverage must start at the top of the content, not at row {}",
            self.coverage.span_start()
        );
        assert_eq!(
            self.coverage.span_end(),
            self.primary_len,
            "invariant 3 (docs/30 §20.3): coverage must reach the end of the content — {} rows covered, content is {} rows",
            self.coverage.span_end(),
            self.primary_len
        );
        assert_eq!(
            self.coverage.rows_covered(),
            self.coverage.span_end() - self.coverage.span_start(),
            "invariant 4 (docs/30 §20.3): the coverage has a hole — {} of the {} rows inside {}..{} were written",
            self.coverage.rows_covered(),
            self.coverage.span_end() - self.coverage.span_start(),
            self.coverage.span_start(),
            self.coverage.span_end()
        );
        assert_eq!(
            tally.committed + tally.discarded,
            tally.step,
            "invariant 5 (docs/30 §20.3): every frame is committed or discarded — {} committed + {} discarded != {} frames",
            tally.committed,
            tally.discarded,
            tally.step
        );
        assert!(
            self.bands.is_disjoint(),
            "invariant 6 (docs/30 §20.3): two bands share a row: {:?}",
            self.bands
                .bands()
                .map(|band| (band.first_row, band.end_row(self.cross_len)))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            self.bands.budget().resident_canvas(),
            self.bands.resident_canvas_bytes(),
            "invariant 7 (docs/30 §20.3, §22.3): the budget says {} canvas bytes are resident and the store holds {} — the memory bound is only real if the accounting is the store's own",
            self.bands.budget().resident_canvas(),
            self.bands.resident_canvas_bytes()
        );
        assert_eq!(
            self.bands.budget().resident_preview(),
            self.bands.resident_preview_bytes(),
            "invariant 7 (docs/30 §20.3, §22.3): the budget says {} preview bytes are resident and the store holds {} — §22.3 spends the preview's memory first, so it has to be accounted for separately",
            self.bands.budget().resident_preview(),
            self.bands.resident_preview_bytes()
        );
        assert!(
            !self.bands.budget().over_budget(),
            "invariant 7 (docs/30 §20.3): {} resident bytes over a budget of {}",
            self.bands.budget().used(),
            self.bands.budget().total()
        );
        assert_eq!(
            std::thread::current().id(),
            self.owner,
            "invariant 8 (docs/30 §20.3, §21.3): the canvas has exactly one user thread — it was created on {:?}",
            self.owner
        );
    }
}

/// Where the viewport's top edge sits in the canvas' content coordinates, and the three things a
/// confirmed step can therefore be (§17.1, §17.4; `P1.19`).
///
/// This is the whole of `F-02`'s `next_pos = current_pos + signed_delta` made executable: a signed
/// displacement moves the viewport, and where it lands decides which of the three writes a step is.
/// The reference implementation's `offset` is the *viewport's* displacement, so its formula reads
/// `position − offset`; this module keeps the estimator's convention throughout (`d > 0` means the
/// content moved up, §15.1), which makes it `position + d` — one sign convention, one place to get
/// it wrong instead of two.
///
/// Nothing here decides *whether* a step is confirmed — that is the four gates' business (§16.1).
/// This type decides what a confirmed step **does**, which is why `Contained` is a first-class
/// answer rather than an append that happens to write nothing.
///
/// It also owns the undo history (§19.6; `P1.22`), because it is the only thing that knows both the
/// viewport position and the canvas. The history is not a log of pixels: it records the canvas length
/// each write started from, and the canvas is append-only, so undoing is arithmetic — no copy, no
/// re-encode, nothing to keep in sync. That is what makes the feature free, and it is why the marks
/// are only pushed when a step actually **wrote** something.
pub(crate) struct ViewportState {
    position: i64,
    extent: u32,
    history: Vec<StepMark>,
}

/// What one write has to be undone with: the canvas length before it, and how many rows it put in
/// front of the rest (`0` for an append, which is the common case).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StepMark {
    primary_len: u64,
    prepended: u64,
}

impl ViewportState {
    /// The viewport starts at the canvas' origin: the first frame **is** the canvas (§17.1).
    pub(crate) fn new(extent: u32) -> Self {
        Self {
            position: 0,
            extent,
            history: Vec::new(),
        }
    }

    pub(crate) fn position(&self) -> i64 {
        self.position
    }

    pub(crate) fn extent(&self) -> u32 {
        self.extent
    }

    /// Applies one confirmed step: decides prepend / append / contained, writes through the canvas
    /// and advances the viewport. Returns what the step did.
    ///
    /// A step larger than the viewport is refused rather than trimmed: `|d| <= extent` is gate one's
    /// correctness constraint (§16.2), and a step that does not overlap at all has no evidence to
    /// confirm it in the first place (§16.5's `Lost`).
    ///
    /// Every call ends with the budget enforced (§22.3): the canvas protects the reference viewport —
    /// the position the viewport now has, which is what the next step will read (§17.4) — and lets the
    /// store spill everything else. Doing it here rather than in the store is deliberate: the store
    /// knows the bands, only the viewport knows which of them the next step needs.
    pub(crate) fn apply(
        &mut self,
        canvas: &mut RecoveredImage,
        frame: &Observation,
        step: i32,
    ) -> Result<StepWrite, BandError> {
        assert_eq!(
            frame.height(),
            self.extent,
            "the viewport's extent is fixed for the session: a resized frame is a new session, not a step"
        );
        if step == 0 {
            // A duplicate frame (§16.3): nothing is written and the position does not move. The
            // budget is still enforced — this is the one path that can run with no write at all.
            canvas.relieve(Some((self.position, self.extent as u64)))?;
            return Ok(StepWrite::Skipped);
        }
        assert!(
            step.unsigned_abs() <= self.extent,
            "a step of {step} px does not overlap the {}-row viewport at all: §16.5's Lost case is not a write",
            self.extent
        );

        let candidate = self.position + step as i64;
        let max_position = canvas.max_position(self.extent as u64);
        let write = if candidate < 0 {
            let rows = (-candidate) as u64;
            let before = canvas.primary_len();
            let write = canvas.prepend_confirmed(frame, rows);
            self.position = 0;
            self.history.push(StepMark {
                primary_len: before,
                prepended: rows,
            });
            write
        } else if candidate > max_position {
            let rows = (candidate - max_position) as u64;
            let write = canvas.append_confirmed(frame, rows);
            self.position = candidate;
            if let StepWrite::Appended { first_row, .. } = write {
                self.history.push(StepMark {
                    primary_len: first_row,
                    prepended: 0,
                });
            }
            write
        } else {
            let bottom = candidate + self.extent as i64;
            assert!(
                bottom <= canvas.primary_len() as i64,
                "the viewport at {candidate} ends at {bottom}, past the canvas' {} rows, so this is an append and not a contained step",
                canvas.primary_len()
            );
            self.position = candidate;
            StepWrite::Contained
        };
        canvas.relieve(Some((self.position, self.extent as u64)))?;
        Ok(write)
    }

    /// The frame the next step estimates against: the canvas window under the viewport (§27.3's
    /// `materialize_reference`).
    ///
    /// `docs/30` §17.4 and `N3`: the reference is **the canvas**, never the previous frame. A frame
    /// that was refused still shows what the screen showed, and estimating against it would measure
    /// the step *after* the refused one against a picture the canvas never accepted — the same pixels
    /// would be weighed twice and the second time as if they had been committed. Reading the canvas
    /// back is also what makes a contained step work at all: the screen may have moved into rows that
    /// were committed several steps ago (§17.3).
    ///
    /// The region's `top` is the viewport position and its `left` is zero: the canvas is the whole
    /// cross-axis extent, so the reference viewport is always a full-width row band. `qpc` is the
    /// caller's, because the canvas has no clock — it is carried so that two references built from the
    /// same rows at different moments are not mistaken for one observation (§11.1's dedupe).
    pub(crate) fn reference(
        &self,
        canvas: &mut RecoveredImage,
        qpc: i64,
    ) -> Result<Observation, BandError> {
        let position = self.position as u64;
        let pixels = canvas.rows(position, self.extent as u64)?;
        let size = (canvas.cross_len() as u32, self.extent);
        let region = Rect::from_origin_size(
            Point::new(0, self.position as i32),
            size.0 as i32,
            size.1 as i32,
        );
        // The only fallible step is the read: `rows` can fail on a spill that cannot be read, and that
        // is a real `BandError`. The packing check that follows is not a second failure mode — `rows`
        // returns exactly `cross_len * 4 * extent` bytes and `size`/`region` are computed from those
        // same two numbers — so a mismatch means the store and the observation disagree about the
        // shape of what was just read. That is an invariant violation, and this file already asserts
        // invariants rather than inventing error values for them (`apply` does the same for the
        // extent). Mapping it onto `BandError::CorruptBand` would mean filling `expected`/`found` —
        // documented as checksums — with byte counts, which is a lie a debugger would believe.
        Ok(Observation::new(pixels, region, qpc, size, canvas.axis()).expect(
            "the rows read back from the canvas pack exactly as the observation they describe",
        ))
    }

    /// Undoes the most recent write: `true` if there was one, `false` if the history is empty.
    ///
    /// The viewport **position does not move back** (§19.6 lists what an undo restores, and the
    /// position is not on the list). The position is a fact about the screen, not a bookmark into the
    /// canvas: rewinding it while the screen stayed where it is would make the next step append the
    /// rows it can see *after* a canvas that no longer reaches them — a hole. Left alone, the next
    /// step's arithmetic is still correct, because a step is only ever confirmed when the frame
    /// overlaps the canvas, and an overlapping frame's new rows are always `<= extent`.
    ///
    /// It also does not touch `ĝ` (§19.6 constraint 2), and it cannot: `ĝ` lives in `Prior`, which is
    /// not reachable from here. The estimate is a property of the machine; an undo is an intention of
    /// the user, and letting the second write back into the first would poison the P1 prior with
    /// something that is not an observation.
    pub(crate) fn undo_last(&mut self, canvas: &mut RecoveredImage) -> Result<bool, BandError> {
        let Some(mark) = self.history.pop() else {
            return Ok(false);
        };
        if mark.prepended == 0 {
            canvas.trim_to(mark.primary_len)?;
        } else {
            canvas.remove_prefix(mark.prepended)?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BYTES_PER_PIXEL, Band, BandError, BandStore, CoverageMap, LongImageLimits, MemoryBudget,
        RecoveredImage, SPILL_FILE_NAME, StepTally, StepWrite, ViewportState, band_height,
    };
    use crate::geometry::Rect;
    use crate::scroll::displacement::{
        Prior, Scratch, Status, candidates_1d, refine_winner, score_candidates_2d, zero_shift_status,
    };
    use crate::scroll::observation::{Axis, Observation};
    use crate::scroll::testkit::{ScrollScript, StepSpec, Structure, TestImage};
    use std::any::Any;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    const CROSS: u64 = 200;
    const ROWS: u64 = 1000;
    const BAND_ROWS: u64 = 10;
    const BUDGET: u64 = 100_000;

    fn band(first_row: u64, rows: u64) -> Band {
        Band::new(first_row, vec![0u8; (CROSS * 4 * rows) as usize])
    }

    struct State {
        image: RecoveredImage,
        viewport_cross: u64,
        tally: StepTally,
    }

    /// A state that satisfies all eight invariants, so that every case below has exactly one thing
    /// wrong with it.
    fn valid() -> State {
        let mut image = RecoveredImage::new(Axis::Vertical, CROSS, MemoryBudget::with_total(BUDGET));
        image.primary_len = ROWS;
        image.coverage.mark_range(0, ROWS);
        image.bands.set_resident(vec![band(0, BAND_ROWS)]);
        State {
            image,
            viewport_cross: CROSS,
            tally: StepTally {
                step: 3,
                committed: 2,
                discarded: 1,
            },
        }
    }

    fn message(payload: Box<dyn Any + Send>) -> String {
        if let Some(text) = payload.downcast_ref::<String>() {
            text.clone()
        } else if let Some(text) = payload.downcast_ref::<&str>() {
            (*text).to_string()
        } else {
            String::from("<the panic payload was not text>")
        }
    }

    fn check(state: &State) -> String {
        let payload = catch_unwind(AssertUnwindSafe(|| {
            state.image.assert_invariants(state.viewport_cross, state.tally)
        }))
        .expect_err("the violation did not panic: the invariant is not asserted");
        message(payload)
    }

    #[test]
    fn assert_invariants_fires_on_each_of_the_eight_violations() {
        // The baseline has to be clean, otherwise "the violation fired" proves nothing about the
        // check that fired.
        let baseline = valid();
        baseline
            .image
            .assert_invariants(baseline.viewport_cross, baseline.tally);

        let mut cases: Vec<(usize, String)> = Vec::new();

        // 1 — the cross axis is constant for the whole session
        let mut state = valid();
        state.image.cross_len = CROSS + 1;
        cases.push((1, check(&state)));

        // 2 — the covered interval starts at the top of the content
        let mut state = valid();
        state.image.coverage = CoverageMap::new(ROWS);
        state.image.coverage.mark_range(5, ROWS);
        cases.push((2, check(&state)));

        // 3 — and it reaches the end
        let mut state = valid();
        state.image.coverage = CoverageMap::new(ROWS);
        state.image.coverage.mark_range(0, ROWS - 1);
        cases.push((3, check(&state)));

        // 4 — two-dimensional completeness: 0..500 plus 501.. is a hole at row 500. The interval
        // ends are both fine; only the *set* can see this, which is why the map stores one.
        let mut state = valid();
        state.image.coverage = CoverageMap::new(ROWS);
        state.image.coverage.mark_range(0, 500);
        state.image.coverage.mark_range(501, ROWS);
        cases.push((4, check(&state)));

        // 5 — every frame is either committed or discarded
        let mut state = valid();
        state.tally.discarded = 2;
        cases.push((5, check(&state)));

        // 6 — bands do not overlap
        let mut state = valid();
        state
            .image
            .bands
            .set_resident(vec![band(0, BAND_ROWS), band(5, BAND_ROWS)]);
        cases.push((6, check(&state)));

        // 7 — resident bytes stay inside the budget
        let mut state = valid();
        state
            .image
            .bands
            .set_budget(MemoryBudget::with_total((CROSS * 4 * BAND_ROWS) - 1));
        cases.push((7, check(&state)));

        // 8 — the canvas has exactly one user thread (§21.3, §22.5). This is the canvas-level form
        // of the rule `P0.02` measured: the check has to fire on the thread that does not own it.
        let state = valid();
        let payload = std::thread::spawn(move || {
            catch_unwind(AssertUnwindSafe(|| {
                state.image.assert_invariants(state.viewport_cross, state.tally)
            }))
            .expect_err("another thread used the canvas and nothing complained")
        })
        .join()
        .expect("the thread that does not own the canvas must panic, not abort");
        cases.push((8, message(payload)));

        assert_eq!(cases.len(), 8);
        for (index, text) in &cases {
            assert!(
                text.contains(&format!("invariant {index}")),
                "case {index} reported a different invariant: {text}"
            );
        }

        // Eight distinct messages, or the eight cases are really fewer cases.
        let mut texts: Vec<&String> = cases.iter().map(|(_, text)| text).collect();
        texts.sort();
        texts.dedup();
        assert_eq!(
            texts.len(),
            8,
            "two violations produced the same message: {cases:#?}"
        );
    }

    // --- the write path (§17.2, §17.3; task P1.18) ---

    const VIEWPORT: u32 = 900;
    const CROSS_PX: u32 = 320;
    const STEP: i32 = 120;
    const DOC_ROWS: u32 = 1900;
    /// §17.5's budget: eight viewports' worth of pixels.
    const VIEWPORT_BUDGET: u64 = 8 * CROSS_PX as u64 * VIEWPORT as u64 * 4;

    fn mixed() -> [Structure; 5] {
        [
            Structure::Checker { cell: 12 },
            Structure::TextRows { line: 19 },
            Structure::NoiseBlocks { cell: 8 },
            Structure::Gradient,
            Structure::HorizontalBars { period: 19 },
        ]
    }

    /// Two frames of one downward step. The second frame carries a moving region over its **top
    /// 60%** — inside the overlap — so that "kept the first frame's bytes" and "took the second
    /// frame's bytes" are different byte strings. The bottom `STEP` rows (the rows this step
    /// actually adds) are untouched by the overlay.
    fn two_frames() -> (Observation, Observation) {
        let image = TestImage::from_structures(CROSS_PX, DOC_ROWS, 11, 19, &mixed());
        let mut script = ScrollScript::new(
            &image,
            VIEWPORT,
            vec![StepSpec::move_by(STEP).with_dynamic(0.6)],
        );
        let first = script.take(0);
        let second = script.take(1);
        (first, second)
    }

    fn frame_rows(frame: &Observation, first_row: u32, end_row: u32) -> Vec<u8> {
        let row_bytes = (CROSS_PX * 4) as usize;
        frame.pixels()[first_row as usize * row_bytes..end_row as usize * row_bytes].to_vec()
    }

    #[test]
    fn only_the_new_rows_are_written() {
        let (first, second) = two_frames();
        let mut canvas = RecoveredImage::new(Axis::Vertical, CROSS_PX as u64, MemoryBudget::with_total(VIEWPORT_BUDGET));
        canvas.start(&first);

        // The Skip path (§16.3's duplicate detection) writes nothing at all.
        assert_eq!(canvas.append_confirmed(&second, 0), StepWrite::Skipped);
        assert_eq!(canvas.primary_len(), VIEWPORT as u64);
        assert_eq!(canvas.bands().bands().count(), 1);

        assert_eq!(
            canvas.append_confirmed(&second, STEP as u64),
            StepWrite::Appended {
                first_row: VIEWPORT as u64,
                rows: STEP as u64,
            }
        );
        canvas.assert_invariants(
            CROSS_PX as u64,
            StepTally {
                step: 2,
                committed: 2,
                discarded: 0,
            },
        );

        // The rows above the step's reach are the first frame's, untouched.
        assert_eq!(
            canvas.rows(0, STEP as u64).expect("resident"),
            frame_rows(&first, 0, STEP as u32)
        );
        // The new rows are the *bottom* of the second frame: its viewport ends at the new content
        // end, so the content rows `[old_end, new_end)` are the frame's rows `[extent − step, extent)`.
        assert_eq!(
            canvas.rows(VIEWPORT as u64, STEP as u64).expect("resident"),
            frame_rows(&second, VIEWPORT - STEP as u32, VIEWPORT)
        );

        // The overlap is where the two write rules differ, so first prove that they *do* differ
        // here: a row-overwriting composer would have put the second frame's rows 0..780 at content
        // rows 120..900, and the fixture made those two byte strings different.
        let kept = frame_rows(&first, STEP as u32, VIEWPORT);
        let overwritten = frame_rows(&second, 0, VIEWPORT - STEP as u32);
        assert_ne!(
            kept, overwritten,
            "the fixture produced identical bytes in the overlap, so this test cannot tell §17.3's rule from a row overwrite"
        );
        assert_eq!(canvas.rows(STEP as u64, (VIEWPORT - STEP as u32) as u64).expect("resident"), kept);
    }

    #[test]
    fn band_height_never_drops_below_a_quarter_of_the_extent() {
        for extent in [16u32, 32, 100, 300, 900, 2160] {
            for shift in -(extent as i32)..=(extent as i32) {
                let band = band_height(extent, shift) as i64;
                // §17.2's first term: the new frame covers at most half the viewport.
                assert!(
                    band >= (extent / 2) as i64,
                    "extent {extent}, shift {shift}: band {band} is under extent/2"
                );
                // §17.3's verifiability constraint: whatever the step, the overlap keeps a quarter
                // of the viewport, so `gain` and `coverage` are measured on something.
                assert!(
                    band - shift as i64 >= (extent / 4) as i64,
                    "extent {extent}, shift {shift}: the overlap is {} but the band has to leave extent/4 = {}",
                    band - shift as i64,
                    extent / 4
                );
                // And the band has to be big enough to *contain* the rows the step adds.
                assert!(
                    band >= shift.max(0) as i64,
                    "extent {extent}, shift {shift}: the band is smaller than the new content"
                );
            }
        }

        // The two regimes and the crossover between them (extent 900: extent/4 = 225, extent/2 = 450).
        assert_eq!(band_height(900, 0), 450);
        assert_eq!(band_height(900, 225), 450);
        assert_eq!(band_height(900, 226), 451);
        assert_eq!(band_height(900, 900), 1125);
        assert_eq!(band_height(900, -300), 450);
    }

    /// Exit condition ③ in executable form: the `Skip` path is answered **before** the estimator.
    ///
    /// The funnel that puts the duplicate check in front of the estimator is `P3.09`'s; what can be
    /// proved today is that the check costs nothing — `Scratch::builds` is where the estimator's own
    /// work shows up (`P1.13`), and it stays at zero across a duplicate frame while the estimator's
    /// entry point does move it.
    #[test]
    fn a_duplicate_frame_is_answered_without_touching_the_estimator() {
        let image = TestImage::from_structures(CROSS_PX, DOC_ROWS, 11, 19, &mixed());
        let mut script = ScrollScript::new(&image, VIEWPORT, vec![StepSpec::repeat()]);
        let first = script.take(0);
        let duplicate = script.take(1);
        assert_eq!(
            first.pixels(),
            duplicate.pixels(),
            "the fixture did not repeat the frame, so there is nothing to skip"
        );

        let mut scratch = Scratch::new();
        assert_eq!(
            zero_shift_status(&first.view(), &duplicate.view()),
            Status::Confirmed { d: 0 }
        );
        assert_eq!(
            scratch.builds(),
            0,
            "the duplicate check went through the estimator"
        );

        // Counterfactual: the estimator's entry point does build images, so the counter above is
        // measuring the thing it claims to measure.
        let candidates = candidates_1d(&first.view(), &duplicate.view(), 0, 8);
        {
            let views = scratch.pool(&first.view(), &duplicate.view());
            let _ = score_candidates_2d(views.previous(), views.current(), &candidates);
        }
        assert!(
            scratch.builds() > 0,
            "the estimator did no work at all, so the counter proves nothing"
        );
    }

    // --- both directions, and the reference frame (§17.1, §17.4; task P1.19) ---

    /// The document's rows `[first, end)` as one packed buffer — what the canvas must equal when it
    /// has recovered a stretch of the document.
    fn document_rows(image: &TestImage, first: u32, end: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((end - first) as usize * CROSS_PX as usize * 4);
        for y in first..end {
            out.extend_from_slice(image.row(y));
        }
        out
    }

    /// One full pass of the estimator over two observations: layer 1 (`candidates_1d`), layer 2
    /// (`score_candidates_2d`) and layer 3 (`refine_winner`).
    ///
    /// The four gates are `P1.13`'s subject, and assembling them into a decision is the session's
    /// (`P3.09`). What this helper exists for is the **composition**: it insists on the exact integer
    /// so that the drift test fails when the canvas, not the estimator, is what moved.
    fn estimate_once(previous: &Observation, current: &Observation, expected: i32) -> Option<i32> {
        let mut scratch = Scratch::new();
        let candidates = candidates_1d(&previous.view(), &current.view(), expected, 8);
        if candidates.is_empty() {
            return None;
        }
        let scored = {
            let views = scratch.pool(&previous.view(), &current.view());
            score_candidates_2d(views.previous(), views.current(), &candidates)
        };
        let refined = {
            let views = scratch.full_resolution(&previous.view(), &current.view());
            refine_winner(views.previous(), views.current(), &scored)
        };
        refined.map(|winner| winner.d)
    }

    #[test]
    fn scrolling_up_produces_a_prepend_not_a_duplicate() {
        const DOC: u32 = 2400;
        const START: u32 = 400;
        let image = TestImage::from_structures(CROSS_PX, DOC, 11, 19, &mixed());
        // The capture starts **mid-document**. That is the only situation in which a canvas can be
        // extended upward at all: a canvas whose first row is the document's first row has nothing
        // above it, and a step that would go above it is refused by the document, not by us.
        let mut script = ScrollScript::starting_at(
            &image,
            VIEWPORT,
            START,
            vec![StepSpec::move_by(300), StepSpec::move_by(-600)],
        );
        let mut canvas = RecoveredImage::new(Axis::Vertical, CROSS_PX as u64, MemoryBudget::with_total(VIEWPORT_BUDGET));
        let mut viewport = ViewportState::new(VIEWPORT);
        canvas.start(&script.take(0));
        assert_eq!(viewport.position(), 0, "the first frame is the canvas' origin");

        let second = script.take(1);
        assert_eq!(
            viewport.apply(&mut canvas, &second, 300).expect("inside budget"),
            StepWrite::Appended {
                first_row: VIEWPORT as u64,
                rows: 300
            }
        );
        assert_eq!(canvas.primary_len(), 1200);
        let before = canvas.rows(0, 1200).expect("resident");

        let third = script.take(2);
        assert_eq!(
            viewport.apply(&mut canvas, &third, -600).expect("inside budget"),
            StepWrite::Prepended { rows: 300 }
        );
        assert_eq!(
            canvas.primary_len(),
            1500,
            "the canvas grew by the rows that were actually new, not by the step"
        );
        assert_eq!(viewport.position(), 0, "the viewport's top is the canvas' new top");

        // No duplicate: every row that used to be the canvas is still there, 300 rows further down.
        assert_eq!(canvas.rows(300, 1200).expect("resident"), before);
        // The new content is the frame's **first** 300 rows: the frame's viewport now starts at the
        // new top, so "above" is the front of the frame and not the back.
        assert_eq!(canvas.rows(0, 300).expect("resident"), frame_rows(&third, 0, 300));
        // And the canvas is the document, row for row: document rows 100..1600.
        assert_eq!(canvas.rows(0, 1500).expect("resident"), document_rows(&image, 100, 1600));
        canvas.assert_invariants(
            CROSS_PX as u64,
            StepTally {
                step: 2,
                committed: 2,
                discarded: 0,
            },
        );
    }

    #[test]
    fn a_small_rollback_that_is_fully_covered_is_contained() {
        let image = TestImage::from_structures(CROSS_PX, DOC_ROWS, 11, 19, &mixed());
        let mut script = ScrollScript::new(
            &image,
            VIEWPORT,
            vec![StepSpec::move_by(120), StepSpec::move_by(-40)],
        );
        let mut canvas = RecoveredImage::new(Axis::Vertical, CROSS_PX as u64, MemoryBudget::with_total(VIEWPORT_BUDGET));
        let mut viewport = ViewportState::new(VIEWPORT);
        canvas.start(&script.take(0));

        let second = script.take(1);
        assert_eq!(
            viewport.apply(&mut canvas, &second, 120).expect("inside budget"),
            StepWrite::Appended {
                first_row: VIEWPORT as u64,
                rows: 120
            }
        );
        let before = canvas.rows(0, 1020).expect("resident");

        let third = script.take(2);
        assert_ne!(
            frame_rows(&third, 0, VIEWPORT),
            frame_rows(&second, 0, VIEWPORT),
            "the fixture did not move the frame, so Contained would be vacuous"
        );
        assert_eq!(viewport.apply(&mut canvas, &third, -40).expect("inside budget"), StepWrite::Contained);
        assert_eq!(canvas.primary_len(), 1020, "a contained step writes nothing");
        assert_eq!(canvas.rows(0, 1020).expect("resident"), before);
        assert_eq!(viewport.position(), 80, "the viewport moved even though the canvas did not");
        canvas.assert_invariants(
            CROSS_PX as u64,
            StepTally {
                step: 2,
                committed: 2,
                discarded: 0,
            },
        );
    }

    #[test]
    fn one_hundred_confirmed_steps_leave_zero_drift() {
        const STEPS: usize = 100;
        const STEP_PX: u32 = 37;
        const DOC: u32 = STEPS as u32 * STEP_PX + VIEWPORT;
        let image = TestImage::from_structures(CROSS_PX, DOC, 11, 19, &mixed());
        let specs: Vec<StepSpec> = (0..STEPS).map(|_| StepSpec::move_by(STEP_PX as i32)).collect();
        let mut script = ScrollScript::new(&image, VIEWPORT, specs);
        let mut canvas = RecoveredImage::new(Axis::Vertical, CROSS_PX as u64, MemoryBudget::with_total(VIEWPORT_BUDGET));
        canvas.start(&script.take(0));
        let mut viewport = ViewportState::new(VIEWPORT);
        let mut expected = STEP_PX as i32;

        for k in 1..=STEPS {
            let current = script.take(k);
            let position = u64::try_from(viewport.position()).expect("the viewport went negative");
            // The reference is the **canvas**, never the previous frame (§17.4, N3). This is the
            // whole point of the test: if the composition ever wrote the wrong rows, the next step's
            // estimate would be made against them and the error would compound.
            let reference = Observation::new(
                canvas.rows(position, VIEWPORT as u64).expect("resident"),
                Rect::new(0, 0, CROSS_PX as i32, VIEWPORT as i32),
                k as i64,
                (CROSS_PX, VIEWPORT),
                Axis::Vertical,
            )
            .expect("the reference viewport is packed and matches its region");
            let document_first = (k as u32 - 1) * STEP_PX;
            assert_eq!(
                reference.pixels(),
                document_rows(&image, document_first, document_first + VIEWPORT),
                "the canvas stopped being the document at step {k}"
            );

            let d = estimate_once(&reference, &current, expected)
                .unwrap_or_else(|| panic!("step {k}: the estimator had no answer at all"));
            assert_eq!(
                d, STEP_PX as i32,
                "step {k}: the estimator did not recover the scripted step"
            );
            assert_eq!(
                viewport.apply(&mut canvas, &current, d).expect("inside budget"),
                StepWrite::Appended {
                    first_row: VIEWPORT as u64 + (k as u64 - 1) * STEP_PX as u64,
                    rows: STEP_PX as u64,
                }
            );
            expected = d;
        }

        assert_eq!(
            canvas.primary_len(),
            VIEWPORT as u64 + STEPS as u64 * STEP_PX as u64
        );
        assert_eq!(
            canvas.rows(0, canvas.primary_len()).expect("resident"),
            document_rows(&image, 0, DOC),
            "the recovered image drifted away from the document"
        );
        canvas.assert_invariants(
            CROSS_PX as u64,
            StepTally {
                step: STEPS as u64,
                committed: STEPS as u64,
                discarded: 0,
            },
        );
    }

    // --- bounded residency: LRU, spill file, checksum (§17.5, §22.3; task P1.20) ---

    const SPILL_CROSS: u64 = 64;
    const SPILL_ROWS_PER_BAND: u64 = 100;

    fn one_band_bytes() -> u64 {
        SPILL_CROSS * SPILL_ROWS_PER_BAND * BYTES_PER_PIXEL
    }

    /// A band's worth of deterministic bytes: every row is a function of its content row, so a spill
    /// or reload that loses, reorders or truncates a row is visible byte-for-byte.
    fn band_bytes(cross_len: u64, first_row: u64, rows: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity((cross_len * rows * BYTES_PER_PIXEL) as usize);
        for row in 0..rows {
            let y = first_row + row;
            for x in 0..cross_len {
                out.push((y * 31 + x * 7) as u8);
                out.push((y / 3 + x) as u8);
                out.push((x * 13 + y * 5) as u8);
                out.push(0xFF);
            }
        }
        out
    }

    /// A directory no other test shares. `BandStore::in_dir` removes it again when the store drops,
    /// so a failing test does not leave anything behind either.
    fn spill_dir(name: &str) -> std::path::PathBuf {
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "snapclip-bands-{name}-{}-{token}",
            std::process::id()
        ))
    }

    fn store_in(dir: &std::path::Path, total: u64) -> BandStore {
        BandStore::in_dir(SPILL_CROSS, MemoryBudget::with_total(total), dir.to_path_buf())
    }

    /// The row band §17.5 calls "one band": the store's budget is expressed in resident bytes, and
    /// one band of this canvas is what the tests below give it.
    fn spill_band(first_row: u64, rows: u64) -> Band {
        Band::new(first_row, band_bytes(SPILL_CROSS, first_row, rows))
    }

    #[test]
    fn a_one_band_budget_still_produces_a_correct_canvas() {
        const BANDS: u64 = 10;
        let dir = spill_dir("one-band");
        // One band of headroom and nothing protected: the store has to spill almost everything it
        // is given and still answer every read correctly. That is §30.4's "the peak does not grow
        // with the length; after recovery it is byte-exact".
        let mut store = store_in(&dir, one_band_bytes());
        let mut peak = 0;
        for index in 0..BANDS {
            let first_row = index * SPILL_ROWS_PER_BAND;
            store.insert(spill_band(first_row, SPILL_ROWS_PER_BAND));
            store
                .relieve(&[])
                .expect("with nothing protected the budget is always reachable");
            peak = peak.max(store.resident_bytes());
        }
        assert!(
            peak <= one_band_bytes(),
            "resident bytes peaked at {peak}, over the one-band budget of {}",
            one_band_bytes()
        );
        assert!(
            store.spilled().len() as u64 >= BANDS - 1,
            "a one-band budget with {BANDS} bands must spill at least {} of them, spilled {}",
            BANDS - 1,
            store.spilled().len()
        );
        let all = store
            .read_rows(0, BANDS * SPILL_ROWS_PER_BAND)
            .expect("every spilled band comes back");
        assert_eq!(
            all,
            band_bytes(SPILL_CROSS, 0, BANDS * SPILL_ROWS_PER_BAND),
            "the canvas read back differently from what was written"
        );
        // Reading does **not** pull a band back into memory. If it did, one full pass over the image
        // (an export, a preview refresh) would make the whole canvas resident again and the bound
        // would be a function of how the image is read instead of how long it is.
        assert_eq!(
            store.spilled().len() as u64,
            BANDS - 1,
            "reading the canvas changed what is resident"
        );
        assert!(store.resident_bytes() <= one_band_bytes());
    }

    #[test]
    fn the_reference_band_and_the_last_two_confirmed_bands_are_never_evicted() {
        let dir = spill_dir("protected");
        // Four bands, three protected: the budget holds three, so exactly one has to go to disk.
        let mut store = store_in(&dir, 3 * one_band_bytes());
        for index in 0..4u64 {
            store.insert(spill_band(index * SPILL_ROWS_PER_BAND, SPILL_ROWS_PER_BAND));
        }
        let protected = [0, 2 * SPILL_ROWS_PER_BAND, 3 * SPILL_ROWS_PER_BAND];
        store
            .relieve(&protected)
            .expect("the protected set fits, so the budget is reachable");
        for first_row in protected {
            assert!(
                store.is_resident(first_row),
                "band {first_row} was evicted although it is the reference or one of the last two"
            );
        }
        assert!(
            !store.is_resident(SPILL_ROWS_PER_BAND),
            "the one unprotected band should be the one that went to disk"
        );
        assert_eq!(store.spilled().len(), 1);

        // The protected set alone over budget is §22.3's third step: the viewport is too big for the
        // budget, which is a `MemoryLimit` and not a silent overrun.
        let tiny_dir = spill_dir("protected-too-small");
        let mut tiny = store_in(&tiny_dir, one_band_bytes());
        tiny.insert(spill_band(0, SPILL_ROWS_PER_BAND));
        tiny.insert(spill_band(SPILL_ROWS_PER_BAND, SPILL_ROWS_PER_BAND));
        assert_eq!(
            tiny.relieve(&[0, SPILL_ROWS_PER_BAND]),
            Err(BandError::MemoryLimit {
                budget: one_band_bytes(),
                protected: 2 * one_band_bytes(),
            }),
            "a budget that cannot hold the protected bands must say so"
        );
    }

    #[test]
    fn a_corrupted_spill_file_is_detected() {
        let dir = spill_dir("corrupt");
        let mut store = store_in(&dir, one_band_bytes());
        store.insert(spill_band(0, SPILL_ROWS_PER_BAND));
        store.insert(spill_band(SPILL_ROWS_PER_BAND, SPILL_ROWS_PER_BAND));
        store
            .relieve(&[SPILL_ROWS_PER_BAND])
            .expect("the second band is protected, so the first one spills");
        assert_eq!(store.spilled().len(), 1, "band 0 should be on disk");

        let offset = store
            .spilled()
            .get(&0)
            .expect("band 0 is the one that spilled")
            .offset;
        {
            use std::io::{Read, Seek, SeekFrom, Write};
            let path = dir.join(SPILL_FILE_NAME);
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .expect("the spill file exists");
            let mut byte = [0u8; 1];
            file.seek(SeekFrom::Start(offset)).expect("seek into the spill");
            file.read_exact(&mut byte).expect("read the byte to flip");
            file.seek(SeekFrom::Start(offset)).expect("seek back");
            file.write_all(&[byte[0] ^ 0xFF]).expect("flip one byte");
        }

        let error = store
            .read_rows(0, SPILL_ROWS_PER_BAND)
            .expect_err("a band whose bytes changed on disk must not be handed back");
        assert_eq!(
            error,
            BandError::CorruptBand {
                first_row: 0,
                expected: store
                    .spilled()
                    .get(&0)
                    .expect("the entry stays in the map")
                    .fnv,
                found: error_checksum(&error),
            },
            "the error has to name the band and both checksums"
        );

        // G12: the failure is visible, and the canvas is **not** cleared — the resident band still
        // reads back, so a session that stops here can still export what it has.
        assert_eq!(store.spilled().len(), 1, "a corrupt band is not silently dropped");
        assert_eq!(
            store
                .read_rows(SPILL_ROWS_PER_BAND, SPILL_ROWS_PER_BAND)
                .expect("the resident band is untouched"),
            band_bytes(SPILL_CROSS, SPILL_ROWS_PER_BAND, SPILL_ROWS_PER_BAND)
        );
    }

    /// The checksum a `CorruptBand` reports as `found`, read back out of the error itself. It is a
    /// helper so the test can assert "the error carries *some* other checksum" without the test
    /// having to recompute the corrupted bytes.
    fn error_checksum(error: &BandError) -> u64 {
        match error {
            BandError::CorruptBand { found, .. } => *found,
            other => panic!("expected a CorruptBand, got {other:?}"),
        }
    }

    /// `P4.06`'s RED. The spill file is a **session-scoped** object (§17.5 ⑥): one file, appended to,
    /// deleted when the session ends. But a session can also end *early* — the last band that pointed
    /// into the file comes back to memory when a straddling band is truncated (`truncate`, §17.6
    /// layer 1) — and from that moment the bytes on disk are a cost nobody is paying for.
    ///
    /// The taskbook phrased this as "the file is removed when its band is evicted back to memory",
    /// which is the per-band file model §22.7 does **not** use: the file holds every spilled band, so
    /// it can only go when the last one leaves. That is what this case pins.
    #[test]
    fn a_spill_file_is_removed_when_its_band_is_evicted_back_to_memory() {
        let dir = spill_dir("reclaim");
        let mut store = store_in(&dir, one_band_bytes());
        store.insert(spill_band(0, SPILL_ROWS_PER_BAND));
        store.insert(spill_band(SPILL_ROWS_PER_BAND, SPILL_ROWS_PER_BAND));
        store
            .relieve(&[SPILL_ROWS_PER_BAND])
            .expect("the second band is protected, so the first one spills");
        let path = dir.join(SPILL_FILE_NAME);
        assert_eq!(store.spilled().len(), 1, "band 0 should be on disk");
        assert!(path.exists(), "a spilled band means the file is there");

        // Half of the spilled band is inside the prefix, so `truncate` reads it back out of the file
        // and re-inserts the kept half as resident (§17.5 ⑤ — a `SpillRef`'s checksum covers exactly
        // the bytes it points at, so the reference cannot simply be narrowed).
        store
            .truncate(SPILL_ROWS_PER_BAND / 2)
            .expect("the straddling band comes back through the checksum");
        assert!(
            store.spilled().is_empty(),
            "the only band that pointed into the file came back to memory"
        );
        assert!(
            !path.exists(),
            "the spill file outlived the last band that pointed into it: {} is still on disk",
            path.display()
        );
    }

    /// `P4.06`'s second RED: the spill file's size has to be **readable**, not just tracked.
    ///
    /// `E-MEM-1` (§23.1) has a column for it, and §22.6's methodology discipline (§6's benchmark
    /// wording) says it cannot be derived from the heap numbers: `allocated` is heap traffic and
    /// `peak` is live heap, so bytes that went to a file show up as a *smaller* footprint while
    /// nothing was actually reclaimed. A number the probe cannot ask for is a number the probe will
    /// eventually invent, so the store answers for itself.
    #[test]
    fn the_spill_file_size_is_recorded_separately() {
        let dir = spill_dir("size");
        let mut store = store_in(&dir, one_band_bytes());
        assert_eq!(
            store.spill_file_bytes(),
            0,
            "a store that never evicted anything has no spill file"
        );
        store.insert(spill_band(0, SPILL_ROWS_PER_BAND));
        store.insert(spill_band(SPILL_ROWS_PER_BAND, SPILL_ROWS_PER_BAND));
        store
            .relieve(&[SPILL_ROWS_PER_BAND])
            .expect("the second band is protected, so the first one spills");

        let spilled: u64 = store.spilled().values().map(|entry| entry.len).sum();
        assert!(spilled > 0, "the case is vacuous unless something spilled");
        assert_eq!(
            store.spill_file_bytes(),
            spilled,
            "the reported size is the file's, and the file holds exactly the spilled bands"
        );
        assert_eq!(
            store.spill_file_bytes(),
            std::fs::metadata(dir.join(SPILL_FILE_NAME))
                .expect("the spill file exists while a band points into it")
                .len(),
            "the store's number and the filesystem's number have to be the same number"
        );
    }

    /// `P4.06`'s exit condition ③, as a **regression nail** rather than a RED: `BandStore`'s `Drop`
    /// already existed (`P1.20`) and already removes the file and — when the store made it — the
    /// directory. What was missing was the *proof* that the session path reaches it, which is what
    /// this case is: a session that is abandoned rather than finished.
    ///
    /// "Cancelled" here means what it means in §20.5 — the user asked for no result — so nothing
    /// runs a cleanup routine. The file has to be gone because a `Drop` ran, not because a shutdown
    /// path remembered to call something.
    #[test]
    fn a_cancelled_session_leaves_no_spill_files() {
        let dir = spill_dir("cancelled");
        let mut image = RecoveredImage::in_dir(
            Axis::Vertical,
            SPILL_CROSS,
            MemoryBudget::with_total(one_band_bytes()),
            dir.clone(),
        );
        for index in 0..4u64 {
            image
                .bands
                .insert(spill_band(index * SPILL_ROWS_PER_BAND, SPILL_ROWS_PER_BAND));
        }
        image
            .bands
            .relieve(&[])
            .expect("with nothing protected the budget is always reachable");
        assert!(
            !image.bands.spilled().is_empty(),
            "the case is vacuous unless the session actually spilled"
        );
        assert!(dir.join(SPILL_FILE_NAME).exists());

        drop(image);

        assert!(
            !dir.exists(),
            "an abandoned session left {} behind",
            dir.display()
        );

        // The production constructor picks its own directory, so this half is the one a real session
        // takes: it is not enough that `in_dir` cleans up when the caller chose the path.
        let mut store = BandStore::new(SPILL_CROSS, MemoryBudget::with_total(one_band_bytes()));
        store.insert(spill_band(0, SPILL_ROWS_PER_BAND));
        store.insert(spill_band(SPILL_ROWS_PER_BAND, SPILL_ROWS_PER_BAND));
        store
            .relieve(&[SPILL_ROWS_PER_BAND])
            .expect("the second band is protected, so the first one spills");
        let production_dir = store.dir.clone();
        assert!(production_dir.join(SPILL_FILE_NAME).exists());
        drop(store);
        assert!(
            !production_dir.exists(),
            "`BandStore::new` left {} behind",
            production_dir.display()
        );
    }

    #[test]
    fn every_memory_budget_field_is_read() {
        let mut budget = MemoryBudget::with_total(1_000);
        assert_eq!(budget.total(), 1_000);
        assert_eq!(budget.resident_canvas(), 0);
        assert_eq!(budget.resident_preview(), 0);
        assert!(!budget.over_budget(), "an empty budget is not over itself");
        budget.set_canvas(1_000);
        assert!(!budget.over_budget(), "exactly at the budget is still inside it");
        assert_eq!(budget.headroom(), 0);
        budget.set_canvas(1_001);
        assert!(budget.over_budget(), "`total` and `resident_canvas` are both read");
        budget.set_canvas(0);
        assert!(!budget.over_budget());
        budget.set_preview(1_001);
        assert!(budget.over_budget(), "`resident_preview` is read too");
        budget.set_preview(0);
        assert_eq!(budget.used(), 0);

        // §22.3's default: 8 viewports of BGRA, because ①–⑤ come to ≈3.1 and the rest is headroom.
        assert_eq!(MemoryBudget::VIEWPORTS, 8);
        assert_eq!(
            MemoryBudget::for_viewport(100, 50).total(),
            100 * 50 * BYTES_PER_PIXEL * 8
        );
    }

    #[test]
    fn a_spilling_canvas_survives_a_long_session() {
        // The wiring test: a canvas whose budget cannot hold the whole session still reads back
        // byte-exact, because the reference viewport and the last two writes stay resident (§17.5)
        // and everything else goes to the spill file and comes back verified.
        const STEPS: u32 = 40;
        const STEP_PX: u32 = 37;
        const VIEWPORT: u32 = 300;
        const DOC: u32 = STEPS * STEP_PX + VIEWPORT;
        let image = TestImage::from_structures(CROSS_PX, DOC, 11, 19, &mixed());
        let mut script = ScrollScript::new(
            &image,
            VIEWPORT,
            (0..STEPS).map(|_| StepSpec::move_by(STEP_PX as i32)).collect(),
        );
        let dir = spill_dir("long-session");
        let budget = MemoryBudget::with_total(CROSS_PX as u64 * 600 * BYTES_PER_PIXEL);
        let mut canvas = RecoveredImage::in_dir(
            Axis::Vertical,
            CROSS_PX as u64,
            budget,
            dir.clone(),
        );
        let mut viewport = ViewportState::new(VIEWPORT);
        canvas.start(&script.take(0));

        for k in 1..=STEPS {
            let previous = canvas
                .rows(viewport.position() as u64, VIEWPORT as u64)
                .expect("the reference viewport materializes");
            let current = script.take(k as usize);
            let reference = Observation::new(
                previous,
                Rect::new(0, 0, CROSS_PX as i32, VIEWPORT as i32),
                k as i64,
                (CROSS_PX, VIEWPORT),
                Axis::Vertical,
            )
            .expect("the reference viewport is packed and matches its region");
            let d = estimate_once(&reference, &current, STEP_PX as i32)
                .unwrap_or_else(|| panic!("step {k}: the estimator had no answer"));
            assert_eq!(d, STEP_PX as i32, "step {k}: the estimator did not recover the step");
            viewport
                .apply(&mut canvas, &current, d)
                .expect("the budget stays reachable: the reference and the last two stay resident");
            assert!(
                canvas.bands().resident_bytes() <= budget.total(),
                "step {k}: {} resident bytes over a budget of {}",
                canvas.bands().resident_bytes(),
                budget.total()
            );
        }

        assert!(
            canvas.bands().spilled().len() > 0,
            "a {}-row budget over {} rows never spilled, so this test proves nothing about spilling",
            budget.total() / (CROSS_PX as u64 * BYTES_PER_PIXEL),
            canvas.primary_len()
        );
        assert_eq!(
            canvas
                .rows(0, canvas.primary_len())
                .expect("the whole canvas comes back"),
            document_rows(&image, 0, DOC),
            "the spilled canvas drifted away from the document"
        );
        canvas.assert_invariants(
            CROSS_PX as u64,
            StepTally {
                step: STEPS as u64,
                committed: STEPS as u64,
                discarded: 0,
            },
        );
    }

    // --- the three layers of §17.6: an injectable limit, a warn threshold, a contiguous prefix
    // (task P1.21) ---

    /// A stand-in for `RowBandSink` (§17.7) whose only job is to prove the trimmed prefix can be
    /// handed to a **streaming** consumer: strictly increasing rows, no reordering, no holes.
    ///
    /// The real sink (`P4.01`/`P4.02`) is what turns these rows into a PNG — and `png` is not a
    /// dependency of this crate (§28.4), so "the artifact decodes back" is asserted there, not here.
    /// What capture owes the encoder is a prefix it can stream without materialising the image.
    struct PrefixSink {
        next_row: u64,
        bytes: Vec<u8>,
    }

    impl PrefixSink {
        fn new() -> Self {
            Self {
                next_row: 0,
                bytes: Vec::new(),
            }
        }

        fn write_rows(&mut self, first_row: u64, rows: &[u8]) -> Result<(), String> {
            if first_row != self.next_row {
                return Err(format!(
                    "write_rows({first_row}) is out of order: the sink is at {}",
                    self.next_row
                ));
            }
            self.bytes.extend_from_slice(rows);
            self.next_row = first_row + (rows.len() / (CROSS_PX as usize * 4)) as u64;
            Ok(())
        }
    }

    /// Drives a canvas until the injected architecture limit stops it — exactly the way the session
    /// will (§20.1: after every confirmed step, ask the limit; the limit is the canvas' policy, not
    /// the estimator's business). Returns the partial, how many rows the trim dropped, and the canvas.
    fn drive_to_the_limit(
        image: &TestImage,
        limits: &LongImageLimits,
        steps: u32,
    ) -> (u64, u64, Vec<u8>, RecoveredImage) {
        let scripted: Vec<StepSpec> = (0..steps).map(|_| StepSpec::move_by(STEP)).collect();
        let mut script = ScrollScript::new(image, VIEWPORT, scripted);
        let mut canvas = RecoveredImage::new(
            Axis::Vertical,
            CROSS_PX as u64,
            MemoryBudget::with_total(VIEWPORT_BUDGET),
        );
        let mut viewport = ViewportState::new(VIEWPORT);
        canvas.start(&script.take(0));

        let mut trimmed = 0;
        for k in 1..=steps {
            if limits.reached(canvas.primary_len(), CROSS_PX as u64) {
                break;
            }
            let current = script.take(k as usize);
            viewport
                .apply(&mut canvas, &current, STEP)
                .expect("the injected budget is a limit on length, not on residency");
            if limits.reached(canvas.primary_len(), CROSS_PX as u64) {
                trimmed = canvas
                    .trim_to(limits.row_limit(CROSS_PX as u64))
                    .expect("the prefix is readable, resident or spilled");
                break;
            }
        }

        let bytes = canvas
            .rows(0, canvas.primary_len())
            .expect("the partial is readable");
        (canvas.primary_len(), trimmed, bytes, canvas)
    }

    #[test]
    fn the_architecture_limit_is_injectable_and_trims_to_a_valid_partial() {
        // §17.6 layer 1 is a **policy value**, not a constant: inject one tenth of a realistic
        // budget and the canvas stops at a contiguous prefix instead of failing.
        let image = TestImage::from_structures(CROSS_PX, DOC_ROWS, 11, 19, &mixed());
        let limits = LongImageLimits::with_max_pixels(CROSS_PX as u64 * 1000);

        assert_eq!(
            limits.row_limit(CROSS_PX as u64),
            1000,
            "the pixel budget has to be converted to rows through the canvas width"
        );
        assert!(!limits.reached(999, CROSS_PX as u64));
        assert!(limits.reached(1000, CROSS_PX as u64));
        assert_eq!(
            LongImageLimits::default().max_pixels(),
            u32::MAX as u64 / 2,
            "the default is the u32 size domain (docs/30 §17.6 layer 1)"
        );
        assert_eq!(
            LongImageLimits::default().warn_length(),
            29_000,
            "the warn threshold is deliberately aligned with PixPin (docs/30 §17.6 layer 2)"
        );

        let (len, trimmed, bytes, canvas) = drive_to_the_limit(&image, &limits, 8);

        assert_eq!(len, 1000, "the canvas must stop at the injected limit");
        assert_eq!(
            trimmed, 20,
            "the step that crossed the limit wrote 120 rows onto 900: the trim gives back 20"
        );
        // The partial **is** the document's prefix. This is what makes the eventual PNG decodable:
        // the rows are the content's own, in order, with no hole and no filler.
        assert_eq!(
            bytes,
            document_rows(&image, 0, 1000),
            "the partial is not the document's prefix"
        );
        canvas.assert_invariants(
            CROSS_PX as u64,
            StepTally {
                step: 2,
                committed: 2,
                discarded: 0,
            },
        );

        // The tail is gone, not merely hidden: no band and no spill entry reaches past the limit.
        assert!(
            canvas
                .bands()
                .bands()
                .all(|band| band.end_row(CROSS_PX as u64) <= 1000),
            "a band still reaches past the trimmed prefix"
        );
        assert!(
            canvas.bands().spilled().keys().all(|first_row| *first_row < 1000),
            "a spilled entry still reaches past the trimmed prefix"
        );

        // And it streams: §17.7's port only accepts strictly increasing rows, so a prefix that is
        // contiguous and ordered is exactly the contract the encoder needs.
        let mut sink = PrefixSink::new();
        sink.write_rows(0, &bytes).expect("the prefix is in order");
        assert_eq!(sink.next_row, 1000);
        assert_eq!(sink.bytes, bytes);
        assert!(
            sink.write_rows(0, &bytes).is_err(),
            "the sink accepted a replayed band, so this test cannot tell a prefix from a patchwork"
        );

        // The limit is injectable, not always on: the default would not have stopped this canvas.
        assert!(!LongImageLimits::default().reached(1860, CROSS_PX as u64));
    }

    #[test]
    fn the_warn_length_is_a_pure_ui_parameter() {
        let image = TestImage::from_structures(CROSS_PX, DOC_ROWS, 11, 19, &mixed());
        let base = LongImageLimits::with_max_pixels(CROSS_PX as u64 * 1000);
        let chatty = base.with_warn_length(100);

        assert_eq!(base.warn_length(), 29_000);
        assert_eq!(chatty.warn_length(), 100);
        // Same architecture limit, same answers: the threshold is not an input to the trim.
        assert_eq!(base.row_limit(CROSS_PX as u64), chatty.row_limit(CROSS_PX as u64));
        for length in [0, 99, 100, 1000, 29_000, 100_000] {
            assert_eq!(
                base.reached(length, CROSS_PX as u64),
                chatty.reached(length, CROSS_PX as u64),
                "the warn threshold changed the architecture verdict at {length} rows"
            );
        }
        assert!(chatty.warns_at(100), "the threshold is inclusive");
        assert!(!chatty.warns_at(99));
        assert!(!base.warns_at(100), "29,000 px is not reached at 100 rows");
        assert!(base.warns_at(29_000));

        // Two identical sessions that differ **only** in the threshold produce identical pixels.
        let (base_len, base_trimmed, base_bytes, _) = drive_to_the_limit(&image, &base, 8);
        let (chatty_len, chatty_trimmed, chatty_bytes, _) = drive_to_the_limit(&image, &chatty, 8);
        assert_eq!(base_len, chatty_len);
        assert_eq!(base_trimmed, chatty_trimmed);
        assert_eq!(
            base_bytes, chatty_bytes,
            "the warn threshold changed the pixels, so it is not a UI parameter"
        );
    }

    // --- undo: the append-only write order makes it free (§19.6; task P1.22) ---

    /// Ten confirmed appends, then one undo. The canvas has to go back to the ninth step's state —
    /// not "approximately": `primary_len`, the bands, the coverage and the bytes are all checked.
    #[test]
    fn undo_returns_to_the_previous_step_and_deletes_later_bands() {
        const STEPS: u64 = 10;
        let image = TestImage::from_structures(
            CROSS_PX,
            DOC_ROWS + (STEPS * STEP as u64) as u32,
            11,
            19,
            &mixed(),
        );
        let mut script = ScrollScript::new(
            &image,
            VIEWPORT,
            (0..STEPS).map(|_| StepSpec::move_by(STEP)).collect(),
        );
        let mut canvas = RecoveredImage::new(
            Axis::Vertical,
            CROSS_PX as u64,
            MemoryBudget::with_total(VIEWPORT_BUDGET),
        );
        let mut viewport = ViewportState::new(VIEWPORT);
        let first = script.take(0);
        canvas.start(&first);
        for k in 0..STEPS {
            let frame = script.take(k as usize + 1);
            viewport
                .apply(&mut canvas, &frame, STEP)
                .expect("inside budget");
        }
        let full = VIEWPORT as u64 + STEPS * STEP as u64;
        assert_eq!(canvas.primary_len(), full);
        assert_eq!(canvas.rows(0, full).expect("resident"), document_rows(&image, 0, full as u32));

        assert!(
            viewport.undo_last(&mut canvas).expect("resident bands"),
            "ten committed steps leave nine things to undo"
        );

        let ninth = VIEWPORT as u64 + (STEPS - 1) * STEP as u64;
        assert_eq!(
            canvas.primary_len(),
            ninth,
            "undo steps the canvas back by the last step's rows"
        );
        assert_eq!(
            canvas.rows(0, ninth).expect("resident"),
            document_rows(&image, 0, ninth as u32),
            "the remaining canvas is still the document, row for row"
        );
        assert!(
            canvas
                .bands()
                .bands()
                .all(|band| band.end_row(CROSS_PX as u64) <= ninth),
            "a band past the new end survived the undo: {:?}",
            canvas
                .bands()
                .bands()
                .map(|band| (band.first_row, band.end_row(CROSS_PX as u64)))
                .collect::<Vec<_>>()
        );
        canvas.assert_invariants(
            CROSS_PX as u64,
            StepTally {
                step: STEPS,
                committed: STEPS,
                discarded: 0,
            },
        );
    }

    /// Undo is a **user** action about the canvas; `ĝ` is a **physical** property of the machine
    /// (§19.6 constraint 2). Neither direction of influence is allowed: undo must not learn from the
    /// step it removed, and it must not rewind the estimate to the value the step replaced.
    ///
    /// The structural half of this is that `undo_last` has no `Prior` parameter — there is nothing to
    /// pass it to. This test is the other half: the value has to still be there afterwards.
    #[test]
    fn undo_does_not_touch_the_learned_estimate() {
        let image = TestImage::from_structures(CROSS_PX, DOC_ROWS, 11, 19, &mixed());
        let mut script = ScrollScript::new(
            &image,
            VIEWPORT,
            vec![StepSpec::move_by(STEP), StepSpec::move_by(STEP)],
        );
        let mut canvas = RecoveredImage::new(
            Axis::Vertical,
            CROSS_PX as u64,
            MemoryBudget::with_total(VIEWPORT_BUDGET),
        );
        let mut viewport = ViewportState::new(VIEWPORT);
        let mut prior = Prior::new(100.0);
        canvas.start(&script.take(0));

        viewport
            .apply(&mut canvas, &script.take(1), STEP)
            .expect("inside budget");
        assert!(prior.confirm(1, STEP), "one notch is an observation");
        let after_first = prior.px_per_notch();

        viewport
            .apply(&mut canvas, &script.take(2), STEP)
            .expect("inside budget");
        assert!(prior.confirm(1, STEP));
        let after_second = prior.px_per_notch();
        assert_ne!(
            after_first, after_second,
            "the fixture has to move the estimate, or this test proves nothing"
        );

        assert!(viewport.undo_last(&mut canvas).expect("resident bands"));
        assert_eq!(
            prior.px_per_notch(),
            after_second,
            "undo changed the learned estimate: user intent must not write back into the physical model"
        );
        assert_ne!(
            prior.px_per_notch(),
            after_first,
            "undo rewound the estimate to the value the removed step replaced, which is the same pollution in the other direction"
        );
    }

    /// Exit condition ③: undoing every step has to leave the canvas exactly as `start` left it — and
    /// then say so, rather than reporting a successful undo that removed nothing.
    #[test]
    fn undoing_every_step_returns_the_canvas_to_its_initial_state() {
        const STEPS: u64 = 10;
        let image = TestImage::from_structures(
            CROSS_PX,
            DOC_ROWS + (STEPS * STEP as u64) as u32,
            11,
            19,
            &mixed(),
        );
        let mut script = ScrollScript::new(
            &image,
            VIEWPORT,
            (0..STEPS).map(|_| StepSpec::move_by(STEP)).collect(),
        );
        let mut canvas = RecoveredImage::new(
            Axis::Vertical,
            CROSS_PX as u64,
            MemoryBudget::with_total(VIEWPORT_BUDGET),
        );
        let mut viewport = ViewportState::new(VIEWPORT);
        let first = script.take(0);
        canvas.start(&first);
        let initial = canvas.rows(0, VIEWPORT as u64).expect("resident");
        for k in 0..STEPS {
            let frame = script.take(k as usize + 1);
            viewport
                .apply(&mut canvas, &frame, STEP)
                .expect("inside budget");
        }

        for remaining in (0..STEPS).rev() {
            assert!(viewport.undo_last(&mut canvas).expect("resident bands"));
            assert_eq!(
                canvas.primary_len(),
                VIEWPORT as u64 + remaining * STEP as u64
            );
        }

        assert_eq!(canvas.primary_len(), VIEWPORT as u64, "the canvas is one viewport again");
        assert_eq!(canvas.bands().bands().count(), 1, "the initial canvas is one band");
        assert_eq!(canvas.rows(0, VIEWPORT as u64).expect("resident"), initial);
        assert_eq!(canvas.coverage.span_start(), 0);
        assert_eq!(canvas.coverage.span_end(), VIEWPORT as u64);
        assert_eq!(canvas.coverage.rows_covered(), VIEWPORT as u64);
        canvas.assert_invariants(
            CROSS_PX as u64,
            StepTally {
                step: STEPS,
                committed: STEPS,
                discarded: 0,
            },
        );
        assert!(
            !viewport.undo_last(&mut canvas).expect("resident bands"),
            "there is nothing left to undo, and saying otherwise would let the UI report an undo that did nothing"
        );
        assert_eq!(canvas.primary_len(), VIEWPORT as u64, "a refused undo changes nothing");
    }

    /// A **prepend** is not a rollback of `span_end`, so it needs its own direction: the rows it added
    /// are at the front. Without this, an upward scroll would block every earlier undo, because the
    /// history is LIFO.
    #[test]
    fn undoing_a_prepend_removes_the_rows_it_added() {
        const DOC: u32 = 2400;
        const START: u32 = 400;
        let image = TestImage::from_structures(CROSS_PX, DOC, 11, 19, &mixed());
        let mut script = ScrollScript::starting_at(
            &image,
            VIEWPORT,
            START,
            vec![StepSpec::move_by(300), StepSpec::move_by(-600)],
        );
        let mut canvas = RecoveredImage::new(
            Axis::Vertical,
            CROSS_PX as u64,
            MemoryBudget::with_total(VIEWPORT_BUDGET),
        );
        let mut viewport = ViewportState::new(VIEWPORT);
        canvas.start(&script.take(0));
        let second = script.take(1);
        viewport
            .apply(&mut canvas, &second, 300)
            .expect("inside budget");
        let appended = canvas.rows(0, 1200).expect("resident");
        let third = script.take(2);
        assert_eq!(
            viewport
                .apply(&mut canvas, &third, -600)
                .expect("inside budget"),
            StepWrite::Prepended { rows: 300 }
        );
        assert_eq!(canvas.primary_len(), 1500);

        assert!(viewport.undo_last(&mut canvas).expect("resident bands"));
        assert_eq!(
            canvas.primary_len(),
            1200,
            "undoing a prepend removes the rows at the front, not the rows at the back"
        );
        assert_eq!(
            canvas.rows(0, 1200).expect("resident"),
            appended,
            "the canvas before the prepend has to come back byte for byte"
        );
        assert_eq!(canvas.coverage.span_start(), 0);
        assert_eq!(canvas.coverage.span_end(), 1200);
        assert_eq!(canvas.coverage.rows_covered(), 1200);
        canvas.assert_invariants(
            CROSS_PX as u64,
            StepTally {
                step: 2,
                committed: 2,
                discarded: 0,
            },
        );
    }

    /// §22.3's first eviction step (`P5.02`, ADR-9): a preview band goes before any canvas band,
    /// because it is rebuildable from the canvas and a canvas band is not.
    ///
    /// The budget here holds exactly the three canvas bands, so the only way back under it is to drop
    /// both preview bands — and the reference band, which is not protected here, must survive anyway.
    /// A store that had never learned the difference would spill the least recently written band it
    /// could see (the preview at row 0, then the canvas band at row 10) and fail every assertion
    /// below that is about a canvas band still being resident.
    #[test]
    fn evicting_preview_bands_never_evicts_the_reference_band() {
        const SCALE: u32 = 4;
        let canvas_bytes = 3 * CROSS * BYTES_PER_PIXEL * BAND_ROWS;
        let mut store = BandStore::new(CROSS, MemoryBudget::with_total(canvas_bytes));
        for index in 0..3u64 {
            store.insert(band(index * BAND_ROWS, BAND_ROWS));
        }
        let preview_row_bytes = (CROSS * BYTES_PER_PIXEL / u64::from(SCALE)) as usize;
        for index in [0u64, 2] {
            store.insert(Band::at_scale(
                index * BAND_ROWS,
                SCALE,
                vec![0u8; 5 * preview_row_bytes],
            ));
        }

        assert!(
            store.budget().over_budget(),
            "the fixture has to start over budget or `relieve` has nothing to decide"
        );
        assert_eq!(
            store.relieve(&[0]),
            Ok(2),
            "both preview bands are what has to go: they are the rebuildable ones"
        );
        for index in 0..3u64 {
            assert!(
                store.is_resident(index * BAND_ROWS),
                "canvas band {} was evicted while a preview band was still resident",
                index * BAND_ROWS
            );
        }
        assert_eq!(store.resident_preview_bytes(), 0);
        assert_eq!(store.resident_canvas_bytes(), canvas_bytes);
        assert_eq!(
            store.spilled().len(),
            0,
            "a dropped preview is not a spilled band: there is nothing on disk to read back"
        );
        assert_eq!(store.spill_file_bytes(), 0);
    }
}
