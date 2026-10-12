//! The growing thumbnail beside the selection.
//!
//! An integer box filter: one thumbnail pixel is the mean of a `k × k` block of the canvas, with
//! `k` the smallest factor that fits the canvas into the panel's width. Thumbnail row `t` covers
//! content rows `[t·k, (t+1)·k)` in **content coordinates** (the first frame's top is row 0, a
//! prepend reaches negative rows), so a prepend adds thumbnail rows in front instead of moving the
//! box boundaries, and a canvas edit only recomputes the thumbnail rows it touches: the stitcher's
//! band, `O(band · W)`, never the whole canvas.

#[derive(Debug)]
pub(crate) struct Thumb {
    k: u32,
    src_width: u32,
    width: u32,
    /// Thumbnail index of `pixels` row 0.
    first: i64,
    rows: u32,
    pixels: Vec<u8>,
}

impl Thumb {
    /// `src_width` is the canvas width, `max_width` the widest thumbnail the panel shows (pixels).
    pub(crate) fn new(src_width: u32, max_width: u32) -> Self {
        let k = src_width.div_ceil(max_width.max(1)).max(1);
        Self {
            k,
            src_width,
            width: (src_width / k).max(1),
            first: 0,
            rows: 0,
            pixels: Vec::new(),
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
        for t in dirty_from..dirty_to {
            self.compute(t, canvas, canvas_rows, top);
        }
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

    fn compute(&mut self, t: i64, canvas: &[u8], canvas_rows: u32, top: i64) {
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
        let k = self.k as usize;
        let mut sums = vec![0u32; self.width as usize * 4];
        for y in lo as usize..hi as usize {
            let line = &canvas[y * src_row..(y + 1) * src_row];
            for (x, sum) in sums.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                for pixel in line[x * k * 4..(x + 1) * k * 4].as_chunks::<4>().0 {
                    for c in 0..4 {
                        sum[c] += u32::from(pixel[c]);
                    }
                }
            }
        }
        let n = (hi - lo) as u32 * self.k;
        for (value, sum) in out.iter_mut().zip(&sums) {
            *value = (sum / n) as u8;
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
        assert_eq!(Thumb::new(1241, 240).width(), 206);
        assert_eq!(Thumb::new(200, 240).factor(), 1);
        assert_eq!(Thumb::new(480, 240).factor(), 2);
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
