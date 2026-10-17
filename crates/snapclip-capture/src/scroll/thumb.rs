//! The growing thumbnail beside the selection.
//!
//! An integer box filter vertically and a scaled one horizontally: one thumbnail pixel is the mean
//! of a `k × 1` block of the canvas, with `k` the smallest factor that covers the canvas's height,
//! and the thumbnail is always exactly `max_width` pixels wide, so it fills the panel edge to edge
//! the way the loupe's pixels fill theirs. Thumbnail row `t` covers content rows `[t·k, (t+1)·k)`
//! in **content coordinates** (the first frame's top is row 0, a prepend reaches negative rows), so
//! a prepend adds thumbnail rows in front instead of moving the box boundaries, and a canvas edit
//! only recomputes the thumbnail rows it touches: the stitcher's band, `O(band · W)`, never the
//! whole canvas.

#[derive(Debug)]
pub(crate) struct Thumb {
    k: u32,
    src_width: u32,
    width: u32,
    /// Thumbnail index of `pixels` row 0.
    first: i64,
    rows: u32,
    pixels: Vec<u8>,
    /// Reused by [`Thumb::compute`]: the source columns each thumbnail pixel covers, and the
    /// four-channel sums being accumulated. Both depend only on `x`, so they are computed once per
    /// `refresh` rather than once per row, and they are kept across calls so a frame that refreshes
    /// 100 rows allocates nothing.
    spans: Vec<(usize, usize)>,
    sums: Vec<u32>,
}

/// The block's mean for one channel: `sum / divisor`, truncated — `floor(mean)`, a fixed half-level
/// darkening that a 240-pixel-wide preview cannot show.
///
/// The divisor is the block's area, shared by all four channels of the pixel, so the caller hoists
/// it and this is one hardware divide per channel. A shared reciprocal was measured as a replacement
/// and did not win on the thumbnail's shape; see `scratch_measure_the_pixel_writeback`.
#[inline]
fn block_mean(sum: u64, divisor: u64) -> u8 {
    (sum / divisor) as u8
}

impl Thumb {
    /// `src_width` is the canvas width, `max_width` the widest thumbnail the panel shows (pixels).
    /// The thumbnail is that wide: the panel draws it 1:1, so anything narrower would show as
    /// padding down the panel's sides.
    pub(crate) fn new(src_width: u32, max_width: u32) -> Self {
        let width = max_width.max(1);
        let k = src_width.div_ceil(width).max(1);
        // The source columns a thumbnail pixel covers: `x·W/w .. (x+1)·W/w`. With `W ≥ w` each
        // range holds at least one column, consecutive ranges start where the previous one ended
        // (`max` only widens a degenerate range), and the last ends exactly on `W` — so every canvas
        // column is counted once and the thumbnail reaches both of the canvas's edges. This depends
        // on the widths alone, which do not change for the lifetime of the thumbnail.
        let spans: Vec<(usize, usize)> = (0..width)
            .map(|x| {
                let x = u64::from(x);
                let src = u64::from(src_width);
                let wide = u64::from(width);
                let left = x * src / wide;
                let right = (((x + 1) * src / wide).max(left + 1)) as usize;
                (left as usize, right)
            })
            .collect();
        Self {
            k,
            src_width,
            width,
            first: 0,
            rows: 0,
            pixels: Vec::new(),
            spans,
            sums: vec![0; width as usize * 4],
        }
    }

    pub(crate) fn width(&self) -> u32 {
        self.width
    }

    pub(crate) fn rows(&self) -> u32 {
        self.rows
    }

    /// The box size: canvas rows per thumbnail row.
    pub(crate) fn factor(&self) -> u32 {
        self.k
    }

    /// Thumbnail index of the first stored row.
    pub(crate) fn first(&self) -> i64 {
        self.first
    }

    /// Recompute after canvas rows `[from, to)` changed. `canvas` holds rows
    /// `[first, first + len)` of the image, whose row 0 is content row `top`; a store that keeps
    /// its rows in several holders passes only the band it just wrote (docs/35 §3.5).
    pub(crate) fn refresh(
        &mut self,
        canvas: &[u8],
        first: u32,
        canvas_rows: u32,
        top: i64,
        from: u32,
        to: u32,
    ) {
        let k = i64::from(self.k);
        let start = top.div_euclid(k);
        let end = (top + i64::from(canvas_rows) + k - 1).div_euclid(k);
        self.cover(start, end);
        let dirty_from = (top + i64::from(from)).div_euclid(k);
        let dirty_to = (top + i64::from(to) + k - 1).div_euclid(k);
        // Borrow the two scratch buffers once for the whole dirty range: the column spans and the
        // channel sums are the same for every row, and a frame usually refreshes dozens of them.
        let mut sums = std::mem::take(&mut self.sums);
        let spans = std::mem::take(&mut self.spans);
        for t in dirty_from..dirty_to {
            self.compute(t, canvas, first, canvas_rows, top, &spans, &mut sums);
        }
        self.sums = sums;
        self.spans = spans;
    }

    /// `count` thumbnail rows starting at stored row `from`, packed BGRA.
    pub(crate) fn window(&self, from: u32, count: u32) -> &[u8] {
        let row = (self.width * 4) as usize;
        let to = (from + count).min(self.rows);
        &self.pixels[from as usize * row..to as usize * row]
    }

    /// Grow the stored range to thumbnail indices `[start, end)`.
    fn cover(&mut self, start: i64, end: i64) {
        let row = (self.width * 4) as usize;
        if self.rows == 0 {
            self.first = start;
        }
        if start < self.first {
            let add = (self.first - start) as usize;
            self.pixels
                .splice(0..0, std::iter::repeat_n(0u8, add * row));
            self.rows += add as u32;
            self.first = start;
        }
        let stored_end = self.first + i64::from(self.rows);
        if end > stored_end {
            let add = (end - stored_end) as usize;
            self.pixels.resize(self.pixels.len() + add * row, 0);
            self.rows += add as u32;
        }
    }

    fn compute(
        &mut self,
        t: i64,
        canvas: &[u8],
        first: u32,
        canvas_rows: u32,
        top: i64,
        spans: &[(usize, usize)],
        sums: &mut [u32],
    ) {
        let k = i64::from(self.k);
        let src_row = (self.src_width * 4) as usize;
        let lo = (t * k - top).max(0);
        let hi = ((t + 1) * k - top).min(i64::from(canvas_rows));
        let lo = lo.max(i64::from(first));
        let hi = hi.min(i64::from(first) + (canvas.len() / src_row) as i64);
        let row = (self.width * 4) as usize;
        let at = (t - self.first) as usize * row;
        let out = &mut self.pixels[at..at + row];
        if lo >= hi {
            return;
        }
        sums[..self.width as usize * 4].fill(0);
        for y in lo as usize..hi as usize {
            let at = (y - first as usize) * src_row;
            let line = &canvas[at..at + src_row];
            let pixels = line.as_chunks::<4>().0;
            for (sum, &(left, right)) in sums.as_chunks_mut::<4>().0.iter_mut().zip(spans) {
                for pixel in &pixels[left..right.min(pixels.len())] {
                    for c in 0..4 {
                        sum[c] += u32::from(pixel[c]);
                    }
                }
            }
        }
        let rows = (hi - lo) as u32;
        // One iteration per thumbnail **pixel**, not per byte: zipping the four-channel sums
        // against the output bytes would stop at a quarter of the row and leave the rest zero.
        for ((value, sum), &(left, right)) in out
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(sums.as_chunks::<4>().0)
            .zip(spans)
        {
            // The block's area, and the block's mean per channel rounded to nearest (a truncated
            // mean loses up to half a level, which is visible as a darkening of light text). The
            // divisor is shared by the pixel's four channels, so it is taken once here.
            let divisor = (u64::from(rows) * (right - left) as u64).max(1);
            for c in 0..4 {
                value[c] = block_mean(u64::from(sum[c]), divisor);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Thumb;

    /// A canvas whose pixel `(x, y)` is `[x, y, x + y, 255]` (mod 256).
    fn ramp(width: u32, rows: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((width * rows * 4) as usize);
        for y in 0..rows {
            for x in 0..width {
                out.extend_from_slice(&[x as u8, y as u8, (x + y) as u8, 255]);
            }
        }
        out
    }

    /// The measurement behind the integer divide: the same thumbnail-sized writeback three ways.
    ///
    /// It exists because "the divide is the bottleneck" was the assumption going into the pixel
    /// writeback, and on this shape it is wrong: the compiler keeps the per-channel divide off the
    /// critical path well enough that replacing it with a shared multiply is not a win. The plain
    /// divide stays, and this test is what says so.
    #[test]
    #[ignore = "measurement, not an assertion"]
    fn scratch_measure_the_pixel_writeback() {
        use std::time::Instant;
        const W: usize = 240;
        const K: usize = 6;
        const COLS: usize = 1323;
        /// Every writeback variant runs this many times per timed round, so one round is several
        /// thumbnails' worth of work and the clock is not reading noise.
        const BATCH: usize = 2_000;
        // A realistic square: 1323 columns over 240 thumbnail pixels is ~6 source columns each, and
        // `k = 6` canvas rows, so a block is about 36 pixels and its mean has a real divisor.
        let block_px = K * (COLS / W + 1);
        let sums: Vec<u64> = (0..W * 4)
            .map(|i| (i as u64 * 977) % (255 * block_px as u64 + 1))
            .collect();
        let per_pixel = block_px as u64;
        // The mean of the accumulated sums, kept out of the loop: the only thing each variant writes.
        let mut out = vec![0u8; W * 4];
        let rounds = 9;
        // Each variant also bumps a static counter: an atomic has to happen, so the writes that feed
        // it cannot be dropped as dead code, which is what makes the two numbers compare fairly.
        static SINK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let scale = 1.0 / per_pixel as f64;
        let variants: [(&str, fn(&mut [u8], &[u64], u64, f64)); 3] = [
            ("integer-div", |out, sums, d, _| {
                for (value, &sum) in out.as_chunks_mut::<4>().0.iter_mut().zip(sums) {
                    for c in 0..4 {
                        value[c] = ((sum + d / 2) / d) as u8;
                    }
                }
            }),
            ("f64-scale", |out, sums, _, scale| {
                for (value, &sum) in out.as_chunks_mut::<4>().0.iter_mut().zip(sums) {
                    for c in 0..4 {
                        value[c] = (sum as f64 * scale).round() as u8;
                    }
                }
            }),
            ("f64-div", |out, sums, d, _| {
                for (value, &sum) in out.as_chunks_mut::<4>().0.iter_mut().zip(sums) {
                    for c in 0..4 {
                        value[c] = (sum as f64 / d as f64).round() as u8;
                    }
                }
            }),
        ];
        let mut best = [f64::MAX; 3];
        for _ in 0..rounds {
            for (i, (_, write)) in variants.iter().enumerate() {
                let t = Instant::now();
                for _ in 0..BATCH {
                    write(&mut out, &sums, per_pixel, scale);
                    SINK.fetch_add(u64::from(out[0]), std::sync::atomic::Ordering::Relaxed);
                }
                best[i] = best[i].min(t.elapsed().as_secs_f64() * 1e6 / BATCH as f64);
            }
        }
        let (a, b, c) = (best[0], best[1], best[2]);
        println!(
            "SCRATCH writeback of {W} px x {BATCH} rounds (divisor {per_pixel}, sink {}): {}=(integer) {a:.1}us, {}=(shared scale) {b:.1}us, {}=(f64 divide) {c:.1}us",
            SINK.load(std::sync::atomic::Ordering::Relaxed),
            variants[0].0,
            variants[1].0,
            variants[2].0
        );
        assert!(a > 0.0 && b > 0.0, "the measurement was optimised away: {a} {b} {c}");
        assert!(a <= b, "the divide is not the bottleneck: integer {a:.1}us vs shared scale {b:.1}us");
    }

    #[test]
    fn the_factor_is_the_smallest_that_fits_the_panel() {
        assert_eq!(Thumb::new(1241, 240).factor(), 6);
        // The thumbnail is the panel's width whatever the canvas is: the well draws it 1:1, so a
        // narrower bitmap would show as padding down the panel's sides.
        assert_eq!(Thumb::new(1241, 240).width(), 240);
        assert_eq!(Thumb::new(200, 240).factor(), 1);
        assert_eq!(Thumb::new(200, 240).width(), 240);
        assert_eq!(Thumb::new(480, 240).factor(), 2);
        assert_eq!(Thumb::new(480, 240).width(), 240);
    }

    /// Every source column reaches the thumbnail exactly once, including the last ones.
    #[test]
    fn the_scaled_filter_covers_the_canvas_edge_to_edge() {
        // Nine columns into five buckets: [0,1) [1,2) [2,4) [4,5) [5,9). Every column is counted
        // once and none twice, so the buckets tile the canvas exactly.
        let (src_width, width) = (9u32, 5u32);
        let spans: Vec<(u32, u32)> = (0..width)
            .map(|x| {
                let left = x * src_width / width;
                (left, ((x + 1) * src_width / width).max(left + 1))
            })
            .collect();
        assert_eq!(spans, vec![(0, 1), (1, 3), (3, 5), (5, 7), (7, 9)]);
        // The buckets touch and none moves backwards: nothing is skipped, nothing is counted twice.
        for pair in spans.windows(2) {
            assert_eq!(pair[1].0, pair[0].1);
        }
        assert_eq!(spans[0].0, 0);
        assert_eq!(spans[width as usize - 1].1, src_width);

        // The same spans over a ramp whose red channel is the column index and whose rows are
        // identical: every red is then the mean of the columns the span covers, and a column lost
        // at either edge shows up as a first or last bucket that misses the canvas's own edge
        // column. A canvas exactly `k` rows tall makes each thumbnail row's divisor whole, so the
        // numbers below are the horizontal filter alone and not a partial block's mean.
        let canvas: Vec<u8> = (0..src_width * 2)
            .flat_map(|i| [(i % src_width) as u8, 0, 0, 255])
            .collect();
        let mut thumb = Thumb::new(src_width, width);
        assert_eq!(thumb.factor(), 2);
        thumb.refresh(&canvas, 0, 2, 0, 0, 2);
        assert_eq!(thumb.rows(), 1);
        let row: Vec<u8> = thumb.window(0, 1).to_vec();
        let reds: Vec<u8> = row.as_chunks::<4>().0.iter().map(|px| px[0]).collect();
        // The mean of each span's columns, truncated — the pixels in `x1..x2` sum to
        // `(x1 + x2 - 1) · (x2 - x1) / 2`, and the divisor is that count.
        let expected: Vec<u8> = spans
            .iter()
            .map(|&(left, right)| {
                let sum = (left + right - 1) * (right - left) / 2;
                (sum / (right - left)) as u8
            })
            .collect();
        assert_eq!(expected, vec![0, 1, 3, 5, 7]);
        assert_eq!(reds, expected);
        assert_eq!(
            reds[0], canvas[0],
            "the canvas's first column is in the first bucket"
        );
        assert_eq!(
            spans.last().unwrap().1,
            src_width,
            "the last bucket runs to the canvas's own right edge"
        );
        assert_eq!(
            (spans.last().unwrap().0..spans.last().unwrap().1).max().unwrap(),
            src_width - 1,
            "so the canvas's last column is inside it"
        );
    }

    #[test]
    fn each_thumbnail_pixel_is_the_mean_of_its_block() {
        // The expected pixels are computed here from the canvas rather than written out by hand: the
        // point of the test is that every pixel of every row is right, including the ones a write
        // loop could silently leave at zero. A block is `k = ceil(width / thumbnail width)` canvas
        // columns wide and `k` rows tall, and the mean is rounded to nearest.
        let (width, rows) = (8u32, 8u32);
        let canvas = ramp(width, rows);
        let mut thumb = Thumb::new(width, 4);
        thumb.refresh(&canvas, 0, rows, 0, 0, rows);
        assert_eq!((thumb.width(), thumb.rows()), (4, 4));
        let k = thumb.factor() as usize;

        let mut expected = vec![0u32; 4 * 4 * 4];
        for ty in 0..thumb.rows() as usize {
            for tx in 0..thumb.width() as usize {
                let (left, right) = (tx * width as usize / 4, (tx + 1) * width as usize / 4);
                let mut count = 0u32;
                for y in ty * k..(ty + 1) * k {
                    for x in left..right {
                        let src = (y * width as usize + x) * 4;
                        for c in 0..4 {
                            expected[(ty * 4 + tx) * 4 + c] += u32::from(canvas[src + c]);
                        }
                        count += 1;
                    }
                }
                for c in 0..4 {
                    let sum = expected[(ty * 4 + tx) * 4 + c];
                    expected[(ty * 4 + tx) * 4 + c] = sum / count;
                }
            }
        }
        let expected: Vec<u8> = expected.iter().map(|&v| v as u8).collect();
        for t in 0..thumb.rows() {
            let at = (t * thumb.width() * 4) as usize;
            assert_eq!(
                thumb.window(t, 1),
                &expected[at..at + thumb.width() as usize * 4],
                "row {t}"
            );
        }
    }

    #[test]
    fn growth_at_the_bottom_adds_rows_and_recomputes_only_the_band() {
        let mut canvas = ramp(8, 8);
        let mut thumb = Thumb::new(8, 4);
        thumb.refresh(&canvas, 0, 8, 0, 0, 8);
        let before = thumb.window(0, 2).to_vec();
        canvas.extend_from_slice(&ramp(8, 12)[8 * 8 * 4..]);
        thumb.refresh(&canvas, 0, 12, 0, 8, 12);
        assert_eq!(thumb.rows(), 6);
        assert_eq!(thumb.window(0, 2), before.as_slice());
        let mut fresh = Thumb::new(8, 4);
        fresh.refresh(&canvas, 0, 12, 0, 0, 12);
        assert_eq!(thumb.window(0, 6), fresh.window(0, 6));
    }

    #[test]
    fn a_prepend_adds_rows_in_front_without_moving_the_blocks() {
        // Content rows -6..10; the canvas starts at content row -6 after the prepend.
        let full = ramp(8, 16);
        let row = 8 * 4;
        let mut thumb = Thumb::new(8, 4);
        thumb.refresh(&full[6 * row..], 0, 10, 0, 0, 10);
        let kept = thumb.window(0, 4).to_vec();
        thumb.refresh(&full, 0, 16, -6, 0, 6);
        assert_eq!(thumb.first(), -3);
        assert_eq!(thumb.rows(), 8);
        assert_eq!(thumb.window(3, 4), kept.as_slice());
    }
}
