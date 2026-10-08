//! P0.04 / `E-PERF-2`: which PNG parameters the streaming export should use.
//!
//! `docs/31` P0.04 asks for twelve measurements — `Compression::{Fast, Balanced, High}` ×
//! `Filter::{NoFilter, Sub, Up, Adaptive}` — over one 30,000 px tall synthetic image, with
//! peak memory held to "one row band" and the artifact verified by decoding it back.
//!
//! # 非端口路径（刻意，且与任务书不同）
//!
//! P4.01's `RowBandSink` port does not exist yet, so this probe drives `png` directly
//! instead of going through the capture-side port. `docs/31` P0.04 offers a one-off script
//! under `docs/Temp/` for exactly this case; a committed integration test is used instead,
//! for two reasons: the numbers have to stay reproducible after the fact, and this file is
//! also the evidence P4.02 points at (`apps/snapclip/src/capture/` — the same crate — is
//! where `PngRowBandSink` lands). The deviation is recorded in `docs/31` under `DEV-6`.
//!
//! `png` is a **dev-dependency** here and becomes a normal dependency at P4.02. That
//! promotion is not this probe's decision: `docs/30` §17.7 and `docs/31` P4.01 require
//! `snapclip-capture` to never see `png`, so the encoder can only live in the shell.
//!
//! # How to run
//!
//! One combination per process (`docs/31` P0.04: 独立进程), driven by
//! `tools/p0-04-png-params.ps1`:
//!
//! ```text
//! $env:SNAPCLIP_PERF2_COMBO = 'Balanced/Sub'
//! $env:SNAPCLIP_PERF2_OUT   = 'docs\Temp\perf2-2026-10-08.json'
//! cargo test --release -p snapclip-app --test png_params_probe \
//!   perf2_measures_one_parameter_combination -- --ignored --nocapture
//! ```
//!
//! Every run appends one JSON line to `SNAPCLIP_PERF2_OUT`; the file is JSON Lines, not a
//! JSON document, precisely so that twelve processes can append to it without a merge step.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

// ---------------------------------------------------------------------------
// A counting allocator: `peak - live` is what "one row band" has to be asserted against.
// ---------------------------------------------------------------------------

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Wraps the system allocator and keeps a running live counter plus its high-water mark.
pub struct CountingAllocator;

impl CountingAllocator {
    fn added(size: usize) {
        let live = LIVE.fetch_add(size, Relaxed) + size;
        let _ = PEAK.fetch_max(live, Relaxed);
    }

    fn removed(size: usize) {
        LIVE.fetch_sub(size, Relaxed);
    }

    /// Bytes currently allocated by this test binary.
    pub fn live() -> usize {
        LIVE.load(Relaxed)
    }

    /// Highest live value seen since the last [`CountingAllocator::reset_peak`].
    pub fn peak() -> usize {
        PEAK.load(Relaxed)
    }

    /// Restart the high-water mark at the current live value.
    pub fn reset_peak() {
        PEAK.store(LIVE.load(Relaxed), Relaxed);
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            CountingAllocator::added(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            CountingAllocator::added(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        CountingAllocator::removed(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            CountingAllocator::removed(layout.size());
            CountingAllocator::added(new_size);
        }
        new
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

// ---------------------------------------------------------------------------
// The synthetic image. It has to compress like a screenshot, not like noise: a screenshot
// of text is a few rectangles plus a lot of locally-similar pixels.
// ---------------------------------------------------------------------------

/// A deterministic page-like image: cards with borders, text-like runs, a scrollbar rail.
pub struct SyntheticImage {
    width: u32,
    height: u32,
}

impl SyntheticImage {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Raw bytes the encoder has to be handed, in BGRA order (capture's order).
    pub fn raw_len(&self) -> usize {
        (self.width as usize) * (self.height as usize) * 4
    }

    /// One BGRA row. Pure function of `y`, so it needs no state and no seed.
    pub fn row_bgra(&self, y: u32, out: &mut [u8]) {
        assert_eq!(out.len(), self.width as usize * 4);
        for x in 0..self.width {
            let [b, g, r, a] = self.pixel_bgra(x, y);
            let at = x as usize * 4;
            out[at] = b;
            out[at + 1] = g;
            out[at + 2] = r;
            out[at + 3] = a;
        }
    }

    /// Deterministic, structure-bearing pixel. `y / 36` is a card, text sits in bands
    /// inside it, and a 6 px rail on the right edge mimics a scrollbar.
    fn pixel_bgra(&self, x: u32, y: u32) -> [u8; 4] {
        let card = (y / 36) % 3;
        let base: u8 = match card {
            0 => 0xF6,
            1 => 0xFA,
            _ => 0xEE,
        };
        let noise = (hash(x / 3, y / 3, 0x51ED) & 0x07) as u8;
        let mut luma = base.saturating_sub(noise);

        // Card border.
        if y % 36 == 0 {
            luma = 0xC8;
        }
        // Text-like runs: dense on some rows of the band, absent on others.
        let in_text_band = (y % 36) >= 6 && (y % 36) < 26;
        if in_text_band && hash(x / 2, y / 2, 0x9E37) % 5 < 2 {
            luma = (luma / 3).max(0x18);
        }
        // A vertical accent stripe, so the image is not uniform along x.
        if (x + 7) % 211 < 3 && y % 36 >= 6 && y % 36 < 30 {
            luma = 0x40;
        }
        // Scrollbar rail on the right.
        if x + 6 >= self.width {
            luma = if (y / 24) % 2 == 0 { 0xD0 } else { 0xBC };
        }

        let tint = (hash(x / 5, y / 7, 0x1234) & 0x0F) as u8;
        [luma.saturating_sub(tint / 2), luma, luma.saturating_sub(tint), 0xFF]
    }
}

/// Cheap integer hash; stands in for texture without pulling in a RNG.
fn hash(x: u32, y: u32, salt: u32) -> u32 {
    let mut h = x
        .wrapping_mul(0x9E37_79B1)
        ^ y.wrapping_mul(0x85EB_CA77)
        ^ salt.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_F491);
    h ^ (h >> 13)
}

// ---------------------------------------------------------------------------
// The two quick gates. These run in debug, in the normal test gate.
// ---------------------------------------------------------------------------

/// RED evidence for P0.04: a materializing encoder has to hold the whole image; a streaming
/// one holds rows. The image is 16 MiB, so a 1 MiB ceiling is a 16× margin.
#[test]
fn the_encoder_streams_rows_without_materializing_the_image() {
    let image = SyntheticImage::new(1024, 4096);
    let mut sink = CountingSink::default();

    let before = CountingAllocator::live();
    CountingAllocator::reset_peak();
    encode_streaming(&mut sink, &image, *chosen())
        .expect("streaming encode must succeed into a discarding sink");
    let band = CountingAllocator::peak() - before;

    println!(
        "streaming encode of {}x{}: sink {} bytes, peak-live {} bytes ({:.1} KiB)",
        image.width(),
        image.height(),
        sink.bytes,
        band,
        band as f64 / 1024.0
    );

    assert!(sink.bytes > 0, "the sink saw no bytes at all");
    assert!(
        band < 1024 * 1024,
        "peak-live was {band} bytes for a {} byte image: the encoder is materializing it",
        image.raw_len()
    );
}

/// The chosen parameters have to produce a file that decodes back to the same pixels.
#[test]
fn the_artifact_decodes_back_to_the_expected_pixels() {
    let image = SyntheticImage::new(64, 96);
    let mut png = Vec::new();
    encode_streaming(&mut png, &image, *chosen()).expect("encode");

    let decoded = decode_rgba(&png, raised_decode_limits()).expect("decode back");
    assert_eq!((decoded.width, decoded.height), (64, 96));

    let mut row = vec![0u8; 64 * 4];
    for y in 0..96u32 {
        image.row_bgra(y, &mut row);
        let at = y as usize * 64 * 4;
        for (x, chunk) in row.chunks_exact(4).enumerate() {
            let out = &decoded.rgba[at + x * 4..at + x * 4 + 4];
            assert_eq!(out, [chunk[2], chunk[1], chunk[0], chunk[3]], "at {x},{y}");
        }
    }
}

/// A sink that counts bytes and allocates nothing, so the measurement is the encoder's.
#[derive(Default)]
struct CountingSink {
    bytes: usize,
}

impl std::io::Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bytes += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The encoder under measurement. P4.02 turns this into `PngRowBandSink`; here it is the
// twelve-parameter matrix's subject.
// ---------------------------------------------------------------------------

use png::{Compression, Filter};

/// The twelve combinations `docs/31` P0.04 enumerates.
const COMBINATIONS: [(&str, Compression, Filter); 12] = [
    ("Fast/NoFilter", Compression::Fast, Filter::NoFilter),
    ("Fast/Sub", Compression::Fast, Filter::Sub),
    ("Fast/Up", Compression::Fast, Filter::Up),
    ("Fast/Adaptive", Compression::Fast, Filter::Adaptive),
    ("Balanced/NoFilter", Compression::Balanced, Filter::NoFilter),
    ("Balanced/Sub", Compression::Balanced, Filter::Sub),
    ("Balanced/Up", Compression::Balanced, Filter::Up),
    ("Balanced/Adaptive", Compression::Balanced, Filter::Adaptive),
    ("High/NoFilter", Compression::High, Filter::NoFilter),
    ("High/Sub", Compression::High, Filter::Sub),
    ("High/Up", Compression::High, Filter::Up),
    ("High/Adaptive", Compression::High, Filter::Adaptive),
];

/// The combination P0.04's measurement selected, plus today's path as the baseline row.
#[derive(Clone, Copy)]
enum Probe {
    Streaming {
        compression: Compression,
        filter: Filter,
    },
    /// `snapclip_history::image::encode_rgba_png` — what the app ships today: the whole
    /// image materialized, `image` 0.25.10 defaults (`Balanced + Adaptive`, docs/30 §5).
    ImageCrateDefault,
}

/// The combination this crate should ship: `Balanced + Up`, measured by this probe on
/// 2026-10-08 — 2344 ms p50 / 14.4 MiB / peak 16.5 MiB for 1280×30000, against 311 ms /
/// 34.0 MiB / peak 413.9 MiB for the whole-image path the app ships today. `docs/30` §17.7.1
/// carries the full twelve-row table and both justifications (speed and size).
const CHOSEN: (Compression, Filter) = (Compression::Balanced, Filter::Up);

fn chosen() -> &'static (Compression, Filter) {
    &CHOSEN
}

fn probe_from_name(name: &str) -> Probe {
    if name == "baseline/image" {
        return Probe::ImageCrateDefault;
    }
    for (label, compression, filter) in COMBINATIONS {
        if label == name {
            return Probe::Streaming {
                compression,
                filter,
            };
        }
    }
    let mut names: Vec<&str> = COMBINATIONS.iter().map(|(label, _, _)| *label).collect();
    names.push("baseline/image");
    panic!("unknown SNAPCLIP_PERF2_COMBO {name:?}; expected one of: {}", names.join(", "));
}

/// Stream `image` into `sink` one row band at a time.
///
/// This is the shape P4.02 ships: `height` is known up front (F-12), each row is filtered and
/// deflated as it arrives, and the only buffers that outlive a row are the two row bands.
/// The BGRA→RGBA swap is per row and inside the measured region, because capture hands us
/// BGRA and PNG has no BGRA color type — that copy is part of the real path, not overhead
/// invented here.
fn encode_streaming<W: std::io::Write>(
    sink: W,
    image: &SyntheticImage,
    (compression, filter): (Compression, Filter),
) -> Result<(), String> {
    use std::io::Write as _;

    let mut encoder = png::Encoder::new(sink, image.width(), image.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(compression);
    encoder.set_filter(filter);

    // `stream_writer` borrows the `Writer`; `into_stream_writer` would demand `W: 'static`,
    // which `&mut Vec<u8>` is not (E0310). The IEND chunk comes from `Writer::finish`.
    let mut writer = encoder
        .write_header()
        .map_err(|error| format!("write_header: {error}"))?;
    let mut stream = writer
        .stream_writer()
        .map_err(|error| format!("stream_writer: {error}"))?;

    let line = image.width() as usize * 4;
    let mut bgra = vec![0u8; line];
    let mut rgba = vec![0u8; line];
    for y in 0..image.height() {
        image.row_bgra(y, &mut bgra);
        bgra_to_rgba(&bgra, &mut rgba);
        stream
            .write_all(&rgba)
            .map_err(|error| format!("row {y}: {error}"))?;
    }
    // The stream closes the zlib data; the writer closes the file.
    stream.finish().map_err(|error| format!("stream finish: {error}"))?;
    writer.finish().map_err(|error| format!("finish: {error}"))
}

/// Swap the channels in place, BGRA in, RGBA out.
fn bgra_to_rgba(bgra: &[u8], rgba: &mut [u8]) {
    for (source, target) in bgra.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
        target[0] = source[2];
        target[1] = source[1];
        target[2] = source[0];
        target[3] = source[3];
    }
}

/// Today's path end to end, so the matrix has a "before" row to compare against.
fn encode_with_the_image_crate(image: &SyntheticImage) -> Vec<u8> {
    let line = image.width() as usize * 4;
    let mut bgra = vec![0u8; line];
    let mut rgba = Vec::with_capacity(image.raw_len());
    for y in 0..image.height() {
        image.row_bgra(y, &mut bgra);
        let start = rgba.len();
        rgba.resize(start + line, 0);
        bgra_to_rgba(&bgra, &mut rgba[start..]);
    }
    snapclip_history::image::encode_rgba_png(&rgba, image.width(), image.height())
        .expect("image crate encode")
}

/// Raised limits for our own artifacts. `png`'s default is 64 MiB, which is smaller than a
/// long screenshot's output buffer (F-10): a `Limits::default()` decode can fail on a file
/// this program produced itself, which is why P4.02 has to raise them explicitly.
fn raised_decode_limits() -> png::Limits {
    png::Limits { bytes: usize::MAX }
}

/// A decoded RGBA frame.
struct Decoded {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn decode_rgba(bytes: &[u8], limits: png::Limits) -> Result<Decoded, String> {
    let decoder = png::Decoder::new_with_limits(std::io::Cursor::new(bytes), limits);
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("read_info: {error}"))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| "output_buffer_size overflowed".to_string())?;
    let mut rgba = vec![0u8; size];
    let info = reader
        .next_frame(&mut rgba)
        .map_err(|error| format!("next_frame: {error}"))?;
    rgba.truncate(info.buffer_size());
    Ok(Decoded {
        width: info.width,
        height: info.height,
        rgba,
    })
}

// ---------------------------------------------------------------------------
// The P0.04 measurement: one combination per process, appended as one JSON line.
// ---------------------------------------------------------------------------

/// Measure one parameter combination. `docs/31` P0.04 wants twelve of these, each in its
/// own process, so that a previous combination's allocator high-water mark and page cache
/// effect cannot show up in the next one's numbers.
#[test]
#[ignore = "P0.04 probe: needs --release and twelve separate processes; see tools/p0-04-png-params.ps1"]
fn perf2_measures_one_parameter_combination() {
    use std::time::Instant;

    let combo = match std::env::var("SNAPCLIP_PERF2_COMBO") {
        Ok(value) => value,
        Err(_) => {
            probe_from_name("");
            unreachable!()
        }
    };
    let probe = probe_from_name(&combo);
    let out = std::path::PathBuf::from(
        std::env::var_os("SNAPCLIP_PERF2_OUT")
            .expect("SNAPCLIP_PERF2_OUT must point at the JSON Lines file to append to"),
    );
    let width = env_u32("SNAPCLIP_PERF2_WIDTH", 1280);
    let height = env_u32("SNAPCLIP_PERF2_HEIGHT", 30_000);
    let runs = env_u32("SNAPCLIP_PERF2_RUNS", 3).max(1) as usize;

    let image = SyntheticImage::new(width, height);
    let line = width as usize * 4;

    let mut encode_ms = Vec::with_capacity(runs);
    let mut png = Vec::new();
    let mut peak_bytes = 0usize;
    for run in 0..runs {
        let live_before = CountingAllocator::live();
        CountingAllocator::reset_peak();
        let started = Instant::now();
        match probe {
            Probe::Streaming {
                compression,
                filter,
            } => {
                png.clear();
                encode_streaming(&mut png, &image, (compression, filter)).expect("encode")
            }
            Probe::ImageCrateDefault => png = encode_with_the_image_crate(&image),
        }
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        encode_ms.push(elapsed);
        // The high-water mark of run 1 carries the sink's growth; later runs reuse it, so
        // the reported peak is the maximum over runs and the sink size is reported too.
        peak_bytes = peak_bytes.max(CountingAllocator::peak().saturating_sub(live_before));
        println!("{combo}: run {run} {elapsed:.0} ms, {} bytes", png.len());
    }

    let png_bytes = png.len();
    let mut sorted = encode_ms.clone();
    sorted.sort_by(f64::total_cmp);
    let p50_ms = sorted[sorted.len() / 2];
    let max_ms = *sorted.last().unwrap();
    let mib_per_s = (image.raw_len() as f64 / (1024.0 * 1024.0)) / (p50_ms / 1000.0);

    // Does the default limit reject our own artifact? This is F-10, measured rather than
    // asserted: whatever it says here goes into docs/30 §17.7.
    let default_limits_ok = decode_rgba(&png, png::Limits::default()).is_ok();
    let decode_started = Instant::now();
    let decoded = decode_rgba(&png, raised_decode_limits()).expect("decode with raised limits");
    let decode_ms = decode_started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!((decoded.width, decoded.height), (width, height), "decoded size");

    // Byte-for-byte: the whole point of streaming is that the file is not merely parseable.
    let mut row = vec![0u8; line];
    let mut rgba_row = vec![0u8; line];
    for y in 0..height {
        image.row_bgra(y, &mut row);
        bgra_to_rgba(&row, &mut rgba_row);
        let at = y as usize * line;
        assert_eq!(
            &decoded.rgba[at..at + line],
            &rgba_row[..],
            "row {y} differs"
        );
    }

    let record = serde_json::json!({
        "kind": "combo",
        "combo": combo,
        "viewport": [width, height],
        "raw_bytes": image.raw_len(),
        "runs": runs,
        "encode_ms": encode_ms,
        "p50_ms": p50_ms,
        "max_ms": max_ms,
        "mib_per_s": mib_per_s,
        "png_bytes": png_bytes,
        "peak_bytes": peak_bytes,
        "decode_ok": true,
        "decode_ms": decode_ms,
        "decodes_under_default_limits": default_limits_ok,
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
    });
    append_json_line(&out, &record);
    println!("{combo}: {record}");
}

fn append_json_line(path: &std::path::Path, value: &serde_json::Value) {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create the JSON Lines directory");
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open the JSON Lines file for append");
    writeln!(file, "{value}").expect("append the measurement");
}

fn env_u32(name: &str, fallback: u32) -> u32 {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be a number, got {value:?}")),
        Err(_) => fallback,
    }
}
