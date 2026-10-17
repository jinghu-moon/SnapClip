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

    /// Copy `rows` whole rows starting at canvas row `first_row` into `out`.
    ///
    /// A store that holds its rows in several places cannot hand out a slice, so this is how a
    /// caller asks for a range it needs (docs/35 §3.5). `out` is exactly `rows × row width` bytes.
    fn read_rows(&self, first_row: u32, rows: u32, out: &mut [u8]);

    /// Hand the whole image over. An in-memory store gives its buffer away, so the caller never
    /// copies it twice; a backing store gathers it from its holders.
    fn take_all(&mut self) -> Vec<u8>;

    /// The budgets this store is responsible for (docs/35 §5.1). Always available: the backing
    /// store is what decides, from these, how much stays in memory.
    fn budgets(&self) -> Budgets;

    /// What the move sites did, for a store that has them (docs/35 §2.2). A backing store moves no
    /// pixels, so it answers `None`.
    #[cfg(feature = "test-support")]
    fn counters(&self) -> Option<MoveCounters> {
        None
    }
}

/// The store phase 1 introduced and the model tests still compare against: one contiguous BGRA
/// buffer, the same `Vec<u8>` the stitcher used to own, with the same growth rule (docs/35 §2.1).
///
/// It is the **oracle**, so it is compiled for tests only: the product uses
/// [`BackedCanvas`](super::backed::BackedCanvas), and an oracle kept in the product would be a
/// second implementation nobody runs.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct MemoryCanvas {
    width: u32,
    max_rows: u32,
    canvas: Vec<u8>,
    rows: u32,
    #[cfg(feature = "test-support")]
    counters: MoveCounters,
}

#[cfg(test)]
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

#[cfg(test)]
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

    fn read_rows(&self, first_row: u32, rows: u32, out: &mut [u8]) {
        let first = first_row as usize * self.row();
        let end = first + rows as usize * self.row();
        out.copy_from_slice(&self.canvas[first..end]);
    }

    fn take_all(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.canvas)
    }

    fn budgets(&self) -> Budgets {
        Budgets {
            pixel_rows: self.rows,
            pixel_bytes: self.canvas.len() as u64,
            working_bytes: self.canvas.capacity() as u64,
            temp_bytes: 0,
        }
    }

    #[cfg(feature = "test-support")]
    fn counters(&self) -> Option<MoveCounters> {
        Some(self.counters)
    }
}

/// A store behind a box is still a store, so a test can drive either implementation through the
/// same code path (the oracle in `stitch`'s model tests does exactly that).
impl CanvasStore for Box<dyn CanvasStore> {
    fn rows(&self) -> u32 {
        (**self).rows()
    }

    fn append(&mut self, growth: u32, band_src: &[u8]) -> bool {
        (**self).append(growth, band_src)
    }

    fn prepend(&mut self, growth: u32, band_src: &[u8]) -> bool {
        (**self).prepend(growth, band_src)
    }

    fn read_rows(&self, first_row: u32, rows: u32, out: &mut [u8]) {
        (**self).read_rows(first_row, rows, out)
    }

    fn take_all(&mut self) -> Vec<u8> {
        (**self).take_all()
    }

    fn budgets(&self) -> Budgets {
        (**self).budgets()
    }

    #[cfg(feature = "test-support")]
    fn counters(&self) -> Option<MoveCounters> {
        (**self).counters()
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
#[cfg(test)]
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
        let mut first = vec![0u8; row];
        store.read_rows(0, 1, &mut first);
        assert_eq!(first, slice[..row], "the prepended row is first");
        let mut rest = vec![0u8; row * H as usize];
        store.read_rows(1, H, &mut rest);
        assert_eq!(rest, slice[row..row * ((H + 1) as usize)]);
    }
}
