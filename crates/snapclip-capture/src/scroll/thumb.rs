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

    /// Recompute after canvas rows `[from, to)` changed. `canvas` holds `canvas_rows` rows whose
    /// row 0 is content row `top`.
    pub(crate) fn refresh(
        &mut self,
        canvas: &[u8],
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
            self.compute(t, canvas, canvas_rows, top, &spans, &mut sums);
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
        canvas_rows: u32,
        top: i64,
        spans: &[(usize, usize)],
        sums: &mut [u32],
    ) {
        let k = i64::from(self.k);
        let src_row = (self.src_width * 4) as usize;
        let lo = (t * k - top).max(0);
        let hi = ((t + 1) * k - top).min(i64::from(canvas_rows));
        let row = (self.width * 4) as usize;
        let at = (t - self.first) as usize * row;
        let out = &mut self.pixels[at..at + row];
        if lo >= hi {
            return;
        }
        sums[..self.width as usize * 4].fill(0);
        for y in lo as usize..hi as usize {
            let line = &canvas[y * src_row..(y + 1) * src_row];
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
            let divisor = (rows * (right - left) as u32).max(1);
            for c in 0..4 {
                value[c] = (sum[c] / divisor) as u8;
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
        thumb.refresh(&canvas, 2, 0, 0, 2);
        assert_eq!(thumb.rows(), 1);
        let row: Vec<u8> = thumb.window(0, 1).to_vec();
        let reds: Vec<u8> = row.as_chunks::<4>().0.iter().map(|px| px[0]).collect();
        let expected: Vec<u8> = spans
            .iter()
            .map(|&(left, right)| ((left + right - 1) / 2) as u8)
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
        let canvas = ramp(8, 8);
        let mut thumb = Thumb::new(8, 4);
        thumb.refresh(&canvas, 8, 0, 0, 8);
        assert_eq!((thumb.width(), thumb.rows()), (4, 4));
        // Block (1, 2): x in 2..4, y in 4..6 → means x = 2.5, y = 4.5, x + y = 7.
        let px = &thumb.window(2, 1)[4..8];
        assert_eq!(px, &[2, 4, 7, 255]);
        // Every pixel of every row, not just the second one of one row: a write loop that runs out
        // of one operand early fills the rest of the row with zeros and only shows up here.
        for t in 0..thumb.rows() {
            let row = thumb.window(t, 1);
            for (x, px) in row.as_chunks::<4>().0.iter().enumerate() {
                let block = (2 * x) as u32;
                assert_eq!(
                    px,
                    &[block as u8, (2 * t) as u8, (block + 2 * t) as u8 + 1, 255],
                    "thumbnail pixel ({x}, {t})"
                );
            }
        }
    }

    #[test]
    fn growth_at_the_bottom_adds_rows_and_recomputes_only_the_band() {
        let mut canvas = ramp(8, 8);
        let mut thumb = Thumb::new(8, 4);
        thumb.refresh(&canvas, 8, 0, 0, 8);
        let before = thumb.window(0, 2).to_vec();
        canvas.extend_from_slice(&ramp(8, 12)[8 * 8 * 4..]);
        thumb.refresh(&canvas, 12, 0, 8, 12);
        assert_eq!(thumb.rows(), 6);
        assert_eq!(thumb.window(0, 2), before.as_slice());
        let mut fresh = Thumb::new(8, 4);
        fresh.refresh(&canvas, 12, 0, 0, 12);
        assert_eq!(thumb.window(0, 6), fresh.window(0, 6));
    }

    #[test]
    fn a_prepend_adds_rows_in_front_without_moving_the_blocks() {
        // Content rows -6..10; the canvas starts at content row -6 after the prepend.
        let full = ramp(8, 16);
        let row = 8 * 4;
        let mut thumb = Thumb::new(8, 4);
        thumb.refresh(&full[6 * row..], 10, 0, 0, 10);
        let kept = thumb.window(0, 4).to_vec();
        thumb.refresh(&full, 16, -6, 0, 6);
        assert_eq!(thumb.first(), -3);
        assert_eq!(thumb.rows(), 8);
        assert_eq!(thumb.window(3, 4), kept.as_slice());
    }
}
