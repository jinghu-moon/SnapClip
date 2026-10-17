//! Where the long image's pixels live, behind one seam (docs/35 phase 1).
//!
//! The stitcher decides *where* a frame goes; a store decides *how* those rows are held. Today the
//! only store is one contiguous canvas, exactly the buffer the stitcher used to own, so this module
//! changes no behaviour: it only gives the placement logic something to talk to other than `Vec<u8>`
//! (docs/35 §2.2, §3.1).
//!
//! The seam carries exactly the operations phase 1 uses. Sealing, row-by-row replay and reading
//! arbitrary ranges arrive with the backing store in phase 2, because an interface ahead of the
//! store that needs it is dead code (docs/35 §8).
//!
//! The budgets are asked of the store rather than computed by callers, because they are not
//! interchangeable (docs/35 §5): pixel scale is how big the image is, working memory is what the
//! process holds, temporary storage is what a backing store may write, and the final file is what
//! an encoder produces. A store answers for the first three; the file size belongs to encoding.

/// The budgets a store can speak for (docs/35 §5.1). The final file size is deliberately absent:
/// compression cannot be predicted from pixel counts, so no store should pretend to know it.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Budgets {
    /// B1: the image itself, in rows.
    pub pixel_rows: u32,
    /// B1: the image itself, in bytes.
    pub pixel_bytes: u64,
    /// B2: what holding those pixels costs this process.
    pub working_bytes: u64,
    /// B3: what a backing store writes to disk (zero for an in-memory store).
    pub temp_bytes: u64,
}

/// How a [`CanvasStore`] holds rows and what it costs to hold them.
pub(crate) trait CanvasStore {
    /// Rows known so far, i.e. the image height.
    fn rows(&self) -> u32;

    /// Grow the bottom by `growth` rows and write `band_src` (the last rows of the newest frame)
    /// over the growing edge. `false` leaves the store untouched.
    fn append(&mut self, growth: u32, band_src: &[u8]) -> bool;

    /// Grow the top by `growth` rows and write `band_src` (the first rows of the newest frame) over
    /// the new top. `false` leaves the store untouched.
    fn prepend(&mut self, growth: u32, band_src: &[u8]) -> bool;

    /// The rows as one contiguous slice when the store happens to have one. A store that keeps its
    /// rows anywhere else returns `None`, and its callers become incremental in phase 3 for the same
    /// reason the thumbnail does (docs/35 §3.4).
    fn row_slice(&self, first_row: u32, rows: u32) -> Option<&[u8]>;

    /// Hand the whole image over. An in-memory store gives its buffer away, so the caller never
    /// copies it twice; a backing store will have to answer differently in phase 2.
    fn take_all(&mut self) -> Vec<u8>;

    /// The budgets this store is responsible for (docs/35 §5.1). Measurement only until phase 2's
    /// backing store needs them to decide what to keep.
    #[cfg(feature = "test-support")]
    fn budgets(&self) -> Budgets;
}

/// The one store there is today: one contiguous BGRA buffer, the same `Vec<u8>` the stitcher used
/// to own, with the same growth rule (docs/35 §2.1).
#[derive(Debug)]
pub(crate) struct MemoryCanvas {
    width: u32,
    max_rows: u32,
    canvas: Vec<u8>,
    rows: u32,
    #[cfg(feature = "test-support")]
    counters: MoveCounters,
}

impl MemoryCanvas {
    /// Start from the first frame.
    pub(crate) fn new(first: &[u8], width: u32, view: u32, max_rows: u32) -> Self {
        debug_assert_eq!(first.len(), (width * view * 4) as usize);
        Self {
            width,
            max_rows: max_rows.max(view),
            canvas: first.to_vec(),
            rows: view,
            #[cfg(feature = "test-support")]
            counters: MoveCounters::default(),
        }
    }

    fn row(&self) -> usize {
        (self.width * 4) as usize
    }

    /// The canvas bytes, for the callers that had a slice before this seam existed.
    #[cfg(test)]
    pub(crate) fn slice(&self) -> &[u8] {
        &self.canvas
    }

    /// The allocation behind the canvas, for the timing tests that watch it double.
    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.canvas.capacity()
    }

    /// Reserve the whole row cap up front, so the attribution experiment runs without a
    /// reallocation (docs/35 §7.1). No product path calls this.
    #[cfg(feature = "test-support")]
    pub(crate) fn preallocate(&mut self, rows: u32) {
        let bytes = self.row() * (rows.min(self.max_rows) as usize);
        let spare = bytes.saturating_sub(self.canvas.len());
        if spare == 0 {
            return;
        }
        if let Err(error) = self.canvas.try_reserve_exact(spare) {
            eprintln!("[snapclip][scroll] the preallocation experiment could not reserve: {error}");
        }
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn counters(&self) -> MoveCounters {
        self.counters
    }

    fn grow_to(&mut self, growth: u32) -> bool {
        // Hoisted: the call below borrows the buffer mutably, so it cannot also read `self`.
        let row = self.row();
        let limit = self.max_rows as usize * row;
        #[cfg(feature = "test-support")]
        {
            let before_len = self.canvas.len();
            let before = self.canvas.as_ptr();
            let started = std::time::Instant::now();
            let ok = grow(&mut self.canvas, growth as usize * row, limit);
            let us = started.elapsed().as_micros() as u64;
            if ok {
                self.counters.grow_us += us;
                if self.canvas.as_ptr() != before {
                    self.counters.grow_moves += 1;
                    self.counters.grow_bytes += before_len as u64;
                }
            }
            ok
        }
        #[cfg(not(feature = "test-support"))]
        {
            grow(&mut self.canvas, growth as usize * row, limit)
        }
    }

    #[cfg(feature = "test-support")]
    fn note_band(&mut self, bytes: usize, started: std::time::Instant) {
        self.counters.band_copies += 1;
        self.counters.band_bytes += bytes as u64;
        self.counters.band_us += started.elapsed().as_micros() as u64;
    }
}

impl CanvasStore for MemoryCanvas {
    fn rows(&self) -> u32 {
        self.rows
    }

    fn append(&mut self, growth: u32, band_src: &[u8]) -> bool {
        if self.rows + growth > self.max_rows || !self.grow_to(growth) {
            return false;
        }
        self.rows += growth;
        let to = self.rows as usize * self.row();
        let band = band_src.len();
        #[cfg(feature = "test-support")]
        let started = std::time::Instant::now();
        self.canvas[to - band..to].copy_from_slice(band_src);
        #[cfg(feature = "test-support")]
        self.note_band(band, started);
        true
    }

    fn prepend(&mut self, growth: u32, band_src: &[u8]) -> bool {
        // The known length is what moves down, so read it before the buffer grows.
        let known = self.canvas.len();
        if self.rows + growth > self.max_rows || !self.grow_to(growth) {
            return false;
        }
        let shift = growth as usize * self.row();
        #[cfg(feature = "test-support")]
        let started = std::time::Instant::now();
        self.canvas.copy_within(..known, shift);
        #[cfg(feature = "test-support")]
        {
            self.counters.shift_moves += 1;
            self.counters.shift_bytes += known as u64;
            self.counters.shift_us += started.elapsed().as_micros() as u64;
        }
        self.rows += growth;
        let band = band_src.len();
        #[cfg(feature = "test-support")]
        let started = std::time::Instant::now();
        self.canvas[..band].copy_from_slice(band_src);
        #[cfg(feature = "test-support")]
        self.note_band(band, started);
        true
    }

    fn row_slice(&self, first_row: u32, rows: u32) -> Option<&[u8]> {
        let first = first_row as usize * self.row();
        let end = first + rows as usize * self.row();
        self.canvas.get(first..end)
    }

    fn take_all(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.canvas)
    }

    #[cfg(feature = "test-support")]
    fn budgets(&self) -> Budgets {
        Budgets {
            pixel_rows: self.rows,
            pixel_bytes: self.canvas.len() as u64,
            working_bytes: self.canvas.capacity() as u64,
            temp_bytes: 0,
        }
    }
}

/// Append `bytes` zeroes to `canvas`, or leave it untouched and return `false` when the memory
/// cannot be had. A long image is the one allocation here that the user's scrolling sizes, and
/// running out must end the growth, not the process (`Vec::resize` aborts on failure).
///
/// Capacity doubles, so a session moves the canvas O(log n) times rather than once per append
/// (each move copies the whole image). `limit` caps only that doubling choice: the call is bounded
/// by `len + bytes` when that is already larger, so this function never refuses a growth the
/// caller's own row cap allowed. The row cap ([`crate::scroll::row_limit`]) is what keeps the
/// canvas inside the byte budget; this is the last line against an allocation the machine cannot
/// serve.
fn grow(canvas: &mut Vec<u8>, bytes: usize, limit: usize) -> bool {
    let needed = canvas.len().saturating_add(bytes);
    let wanted = needed.max(canvas.capacity().saturating_mul(2).min(limit));
    if let Err(error) = canvas.try_reserve_exact(wanted - canvas.len()) {
        eprintln!(
            "[snapclip][scroll] the long image cannot grow by {bytes} bytes past {}: {error}",
            canvas.len()
        );
        return false;
    }
    canvas.resize(canvas.len() + bytes, 0);
    true
}

/// What the three "move a lot of bytes" sites in [`MemoryCanvas`] actually did (docs/35 §2.2,
/// §7.1).
///
/// Compiled only with `test-support`: the product build has no counters, no clock reads and no
/// extra field, so the default path is byte for byte the code it was (docs/35 phase 0).
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MoveCounters {
    /// `grow` calls that had to reallocate (the canvas pointer moved) and how much they copied.
    pub grow_moves: u32,
    pub grow_bytes: u64,
    pub grow_us: u64,
    /// `Prepended` shifts of the whole known canvas (`copy_within`).
    pub shift_moves: u32,
    pub shift_bytes: u64,
    pub shift_us: u64,
    /// `band` overwrites: frame bytes copied into the canvas at the growing edge.
    pub band_copies: u32,
    pub band_bytes: u64,
    pub band_us: u64,
}

#[cfg(test)]
mod tests {
    use super::{CanvasStore, MemoryCanvas, grow};

    const W: u32 = 4;
    const H: u32 = 3;

    fn frame(value: u8) -> Vec<u8> {
        vec![value; (W * H * 4) as usize]
    }

    /// Out of memory is the end of the image, not of the process: the buffer is left as it was.
    #[test]
    fn a_buffer_that_cannot_grow_is_left_untouched() {
        let mut canvas = vec![7u8; 64];
        assert!(!grow(&mut canvas, usize::MAX - 8, usize::MAX));
        assert_eq!(canvas, vec![7u8; 64]);
        assert!(grow(&mut canvas, 16, 96));
        assert!(canvas.capacity() <= 96, "doubling stops at the limit");
        assert_eq!(canvas.len(), 80);
        assert!(canvas[..64].iter().all(|&b| b == 7) && canvas[64..].iter().all(|&b| b == 0));
    }

    /// The budgets a memory store answers: pixel scale is the image, working memory is the
    /// allocation, and there is no temporary storage at all (docs/35 §5.1).
    #[cfg(feature = "test-support")]
    #[test]
    fn the_store_answers_for_its_own_budgets() {
        let mut store = MemoryCanvas::new(&frame(1), W, H, 8 * H);
        let budgets = store.budgets();
        assert_eq!(budgets.pixel_rows, H);
        assert_eq!(budgets.pixel_bytes, (W * H * 4) as u64);
        assert!(budgets.working_bytes >= budgets.pixel_bytes);
        assert_eq!(budgets.temp_bytes, 0, "an in-memory store writes nothing");

        let row = (W * 4) as usize;
        assert!(store.append(2, &frame(2)[..2 * row]));
        let grown = store.budgets();
        assert_eq!(grown.pixel_rows, H + 2);
        assert_eq!(grown.pixel_bytes, (W * (H + 2) * 4) as u64);
    }

    /// Rows go in at the bottom, at the top, and come back out in order.
    #[test]
    fn rows_travel_in_both_directions() {
        let mut store = MemoryCanvas::new(&frame(1), W, H, 8 * H);
        let row = (W * 4) as usize;
        assert!(store.append(2, &frame(2)[..2 * row]));
        assert!(store.prepend(1, &frame(3)[..row]));

        let slice = store.slice();
        assert_eq!(slice.len(), (W * (H + 3) * 4) as usize);
        assert_eq!(
            store.row_slice(0, 1),
            Some(&slice[..row]),
            "the prepended row is first"
        );
        assert_eq!(store.row_slice(1, H), Some(&slice[row..row * ((H + 1) as usize)]));
        assert_eq!(store.row_slice(0, H + 4), None, "past the end is not a slice");
    }
}
