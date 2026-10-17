//! Where the long image's pixels live once it outgrows one buffer (docs/35 phase 2).
//!
//! The stitcher only ever writes the **band** at one growing edge (docs/35 §2.1), and a band is at
//! most one viewport (`stitch.rs`, `band()`). A row further than a viewport from either edge is
//! therefore final: nothing can reach it again. That is what makes this store possible — the rows
//! that can still change live in two bounded ring buffers, one per edge, and everything between
//! them is **sealed** into a tile file whose rows never change again. The directory of tiles is a
//! deque, so a prepend inserts at its front and an append pushes at its back: the *logical* order
//! is the sequence, and the pixels inside the file never move (docs/35 §3.3).
//!
//! What this buys is the memory, not the code:
//!
//! * the edge buffers are allocated once at a fixed capacity (no doubling, so no whole-image
//!   `memcpy` — phase 0 attributed the 16.18 ms beat to exactly that);
//! * a prepend moves no pixels: it grows the front ring and pushes one directory entry;
//! * the rows between the edges are on disk, so holding a 30 000 row image costs two viewports of
//!   RAM plus a file (B2 against B3, docs/35 §5).
//!
//! The front ring is built lazily: a downward session never allocates it, so the common case holds
//! one viewport. The first prepend unseals the top tiles back into the ring, because the band it
//! writes may cover rows that were sealed. Sealing is therefore "final until unsealed", and a row
//! is only sealed when it is more than one viewport from the edge it belongs to — the invariant
//! the model tests in [`stitch`](super::stitch) check against the frozen reference.
//!
//! Temporary storage is a real cost (raw BGRA, so 1280×9300 rows of a long session is ~153 MB of
//! disk). It is counted by [`CanvasStore::budgets`] as B3 and removed when the store is dropped.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use super::store::{Budgets, CanvasStore};

/// A fixed-capacity ring of rows. Logical row `i` lives at physical row `(head + i) % cap`, so
/// growing at either end is index arithmetic and never a move.
#[derive(Debug)]
struct Ring {
    pixels: Vec<u8>,
    cap: u32,
    rows: u32,
    head: u32,
    row: usize,
}

impl Ring {
    fn new(row: usize, cap: u32) -> Self {
        debug_assert!(cap > 0 && row > 0);
        Self {
            pixels: vec![0; row * cap as usize],
            cap,
            rows: 0,
            head: 0,
            row,
        }
    }

    fn physical(&self, logical: u32) -> u32 {
        let sum = self.head + logical;
        if sum >= self.cap {
            sum - self.cap
        } else {
            sum
        }
    }

    /// Copy `src` (whole rows) over logical rows `[first, first + rows)`.
    fn write(&mut self, first: u32, src: &[u8]) {
        let rows = (src.len() / self.row) as u32;
        debug_assert!(first + rows <= self.rows, "write past the ring's rows");
        let mut done = 0;
        while done < rows {
            let at = self.physical(first + done);
            let take = (rows - done).min(self.cap - at);
            let from = done as usize * self.row;
            let to = at as usize * self.row;
            self.pixels[to..to + take as usize * self.row]
                .copy_from_slice(&src[from..from + take as usize * self.row]);
            done += take;
        }
    }

    /// Copy logical rows `[first, first + rows)` into `out` (whole rows).
    fn read(&self, first: u32, rows: u32, out: &mut [u8]) {
        debug_assert!(first + rows <= self.rows, "read past the ring's rows");
        let mut done = 0;
        while done < rows {
            let at = self.physical(first + done);
            let take = (rows - done).min(self.cap - at);
            let from = at as usize * self.row;
            let to = done as usize * self.row;
            out[to..to + take as usize * self.row]
                .copy_from_slice(&self.pixels[from..from + take as usize * self.row]);
            done += take;
        }
    }

    fn grow_front(&mut self, rows: u32) -> bool {
        if self.rows + rows > self.cap {
            return false;
        }
        self.head = (self.head + self.cap - rows % self.cap) % self.cap;
        self.rows += rows;
        true
    }

    fn grow_back(&mut self, rows: u32) -> bool {
        if self.rows + rows > self.cap {
            return false;
        }
        self.rows += rows;
        true
    }

    fn shrink_front(&mut self, rows: u32) {
        debug_assert!(rows <= self.rows);
        self.head = self.physical(rows);
        self.rows -= rows;
    }

    fn shrink_back(&mut self, rows: u32) {
        debug_assert!(rows <= self.rows);
        self.rows -= rows;
    }
}

/// One sealed run of rows in the tile file. `offset` is where its first row starts.
#[derive(Debug, Clone, Copy)]
struct Tile {
    offset: u64,
    rows: u32,
}

/// The sealed middle: one append-only file and a directory that gives it a logical order.
#[derive(Debug)]
struct Tiles {
    file: Option<File>,
    path: PathBuf,
    bytes: u64,
    order: VecDeque<Tile>,
    rows: u32,
}

impl Drop for Tiles {
    fn drop(&mut self) {
        // The file is the store's own; losing it with the store is the contract (docs/35 §5.3).
        self.file.take();
        if let Err(error) = std::fs::remove_file(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "[snapclip][scroll] the tile file {} was left behind: {error}",
                    self.path.display()
                );
            }
        }
    }
}

impl Tiles {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "snapclip-scroll-{}-{serial}.tile",
            std::process::id()
        ));
        Self {
            file: None,
            path,
            bytes: 0,
            order: VecDeque::new(),
            rows: 0,
        }
    }

    /// The file is created on the first seal, so a short session writes nothing at all.
    fn ensure_file(&mut self) -> Result<&mut File, String> {
        if self.file.is_none() {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&self.path)
                .map_err(|error| format!("{}: {error}", self.path.display()))?;
            self.file = Some(file);
        }
        Ok(self.file.as_mut().expect("just created"))
    }

    fn push(&mut self, offset: u64, rows: u32, at_front: bool) {
        if at_front {
            self.order.push_front(Tile { offset, rows });
        } else {
            self.order.push_back(Tile { offset, rows });
        }
        self.rows += rows;
    }
}

/// The store phase 2 hands to the stitcher: bounded rings at the edges, a tile file between them.
#[derive(Debug)]
pub(crate) struct BackedCanvas {
    view: u32,
    max_rows: u32,
    rows: u32,
    row: usize,
    /// Rows `[0, front.rows)`. Absent until a prepend needs the top edge, so a downward session
    /// allocates one viewport rather than two.
    front: Option<Ring>,
    /// The newest rows, `[rows - back.rows, rows)`. Always present.
    back: Ring,
    middle: Tiles,
    /// Row width, for the middle's accounting.
    middle_bytes: u64,
    /// Why the last seal or unseal failed, kept for the `Debug` output and the log line that
    /// already went to stderr (docs/35 §6).
    error: Option<String>,
}

impl BackedCanvas {
    pub(crate) fn new(first: &[u8], width: u32, view: u32, max_rows: u32) -> Self {
        let row = (width * 4) as usize;
        debug_assert_eq!(first.len(), row * view as usize);
        debug_assert!(view > 0);
        let view = view.max(1).min(max_rows.max(1));
        let mut back = Ring::new(row, view);
        back.grow_back(view);
        back.write(0, first);
        Self {
            view,
            max_rows: max_rows.max(view),
            rows: view,
            row,
            front: None,
            back,
            middle: Tiles::new(),
            middle_bytes: 0,
            error: None,
        }
    }

    fn front_rows(&self) -> u32 {
        self.front.as_ref().map_or(0, |front| front.rows)
    }

    fn back_first(&self) -> u32 {
        self.rows - self.back.rows
    }

    /// Rows held in the middle: between the front's rows and the back's.
    fn middle_first(&self) -> u32 {
        self.front_rows()
    }

    /// Read `rows` whole rows starting at canvas row `first` into `out`.
    fn gather(&self, first: u32, rows: u32, out: &mut [u8]) {
        debug_assert_eq!(out.len(), rows as usize * self.row);
        let mut done = 0;
        let front_rows = self.front_rows();
        let middle_first = self.middle_first();
        let middle_end = self.rows - self.back.rows;
        while done < rows {
            let at = first + done;
            if at < front_rows {
                let take = (rows - done).min(front_rows - at);
                let front = self.front.as_ref().expect("front_rows implies a ring");
                front.read(
                    at,
                    take,
                    &mut out[done as usize * self.row..(done + take) as usize * self.row],
                );
                done += take;
            } else if at < middle_end {
                let take = (rows - done).min(middle_end - at);
                self.read_middle(at - middle_first, take, &mut out[done as usize * self.row..]);
                done += take;
            } else {
                let take = (rows - done).min(self.rows - at);
                self.back.read(
                    at - self.back_first(),
                    take,
                    &mut out[done as usize * self.row..],
                );
                done += take;
            }
        }
    }

    fn read_middle(&self, first: u32, rows: u32, out: &mut [u8]) {
        let Some(mut file) = self.readable_file() else {
            out[..rows as usize * self.row].fill(0);
            return;
        };
        let mut skip = first;
        let mut done = 0;
        for tile in &self.middle.order {
            if skip >= tile.rows {
                skip -= tile.rows;
                continue;
            }
            let take = (rows - done).min(tile.rows - skip);
            let offset = tile.offset + u64::from(skip) * self.row as u64;
            let into = &mut out[done as usize * self.row..(done + take) as usize * self.row];
            if let Err(error) = file
                .seek(SeekFrom::Start(offset))
                .and_then(|_| file.read_exact(into))
            {
                eprintln!(
                    "[snapclip][scroll] reading the tile file {} failed: {error}",
                    self.middle.path.display()
                );
                into.fill(0);
            }
            done += take;
            skip = 0;
            if done >= rows {
                break;
            }
        }
        if done < rows {
            out[done as usize * self.row..rows as usize * self.row].fill(0);
        }
    }

    fn readable_file(&self) -> Option<File> {
        if self.middle.order.is_empty() {
            return None;
        }
        match OpenOptions::new().read(true).open(&self.middle.path) {
            Ok(file) => Some(file),
            Err(error) => {
                eprintln!(
                    "[snapclip][scroll] the tile file {} cannot be read back: {error}",
                    self.middle.path.display()
                );
                None
            }
        }
    }

    /// Move `rows` of `ring` into the middle at its front or back, sealing them for good.
    ///
    /// `ring_is_front` says which edge the ring belongs to: a front ring seals from its back
    /// (those rows are more than a viewport below the top), a back ring from its front.
    fn seal_ring(&mut self, ring_is_front: bool, rows: u32) -> bool {
        if rows == 0 {
            return true;
        }
        let mut bytes = vec![0u8; rows as usize * self.row];
        match (&self.front, ring_is_front) {
            (Some(front), true) => front.read(front.rows - rows, rows, &mut bytes),
            _ => self.back.read(0, rows, &mut bytes),
        }
        let offset = self.middle.bytes;
        let written = {
            let file = match self.middle.ensure_file() {
                Ok(file) => file,
                Err(error) => {
                    self.error = Some(error.clone());
                    eprintln!("[snapclip][scroll] the long image cannot seal a tile: {error}");
                    return false;
                }
            };
            file.seek(SeekFrom::Start(offset))
                .and_then(|_| file.write_all(&bytes))
                .map(|_| ())
                .map_err(|error| format!("{}: {error}", self.middle.path.display()))
        };
        match written {
            Ok(()) => {
                // The file only grows: unsealing leaves its bytes behind as dead space, which the
                // budget reports honestly as temporary storage.
                self.middle.bytes = self.middle.bytes.max(offset + bytes.len() as u64);
                self.middle.push(offset, rows, ring_is_front);
                self.middle_bytes += bytes.len() as u64;
                if ring_is_front {
                    if let Some(front) = self.front.as_mut() {
                        front.shrink_back(rows);
                    }
                } else {
                    self.back.shrink_front(rows);
                }
                true
            }
            Err(error) => {
                self.error = Some(error.clone());
                eprintln!("[snapclip][scroll] the long image cannot seal a tile: {error}");
                false
            }
        }
    }

    /// Bring the front ring up to `wanted` rows by unsealing the rows just below it.
    ///
    /// Two sources, in order: the middle's first tile, and — once the middle is empty — the back
    /// ring's first rows, which are then the rows immediately below the front (the image is short
    /// enough that the two rings meet).
    fn unseal_front(&mut self, wanted: u32) -> bool {
        if self.front.is_none() {
            self.front = Some(Ring::new(self.row, self.view));
        }
        let wanted = wanted.min(self.view).min(self.rows);
        while self.front_rows() < wanted {
            let need = wanted - self.front_rows();
            let take = match self.middle.order.front().copied() {
                Some(tile) => {
                    let take = need.min(tile.rows);
                    self.middle.order.pop_front();
                    let mut bytes = vec![0u8; take as usize * self.row];
                    if let Some(mut file) = self.readable_file() {
                        if let Err(error) = file
                            .seek(SeekFrom::Start(tile.offset))
                            .and_then(|_| file.read_exact(&mut bytes))
                        {
                            eprintln!(
                                "[snapclip][scroll] unsealing a tile failed: {error}; its rows read as zero"
                            );
                            bytes.fill(0);
                        }
                    }
                    if take < tile.rows {
                        self.middle.order.push_front(Tile {
                            offset: tile.offset + u64::from(take) * self.row as u64,
                            rows: tile.rows - take,
                        });
                    } else {
                        self.middle.rows -= take;
                    }
                    (bytes, true)
                }
                None => {
                    let take = need.min(self.back.rows);
                    if take == 0 {
                        break;
                    }
                    let mut bytes = vec![0u8; take as usize * self.row];
                    self.back.read(0, take, &mut bytes);
                    self.back.shrink_front(take);
                    (bytes, false)
                }
            };
            let (bytes, from_middle) = take;
            let rows = (bytes.len() / self.row) as u32;
            let front = self.front.as_mut().expect("just ensured");
            if !front.grow_front(rows) {
                // The ring cannot hold it: give up without corrupting either side.
                front.shrink_front(rows.min(front.rows));
                if from_middle {
                    self.middle.order.push_front(Tile {
                        offset: 0,
                        rows,
                    });
                    self.middle.rows += rows;
                } else {
                    self.back.grow_front(rows);
                }
                return self.fail("the front edge buffer is smaller than the band".into());
            }
            front.write(0, &bytes);
        }
        true
    }

    fn fail(&mut self, error: String) -> bool {
        eprintln!("[snapclip][scroll] {error}");
        self.error = Some(error);
        false
    }
}

impl CanvasStore for BackedCanvas {
    fn rows(&self) -> u32 {
        self.rows
    }

    fn append(&mut self, growth: u32, band_src: &[u8]) -> bool {
        if self.rows + growth > self.max_rows {
            return false;
        }
        let band = (band_src.len() / self.row) as u32;
        debug_assert!(band <= self.view && band >= growth, "{band} {growth}");
        // A downward step ends any upward episode: the front ring's rows are final until a prepend
        // unseals them again, so the memory goes back now (docs/35 §5.2).
        if self.front.is_some() {
            let rows = self.front_rows();
            if !self.seal_ring(true, rows) {
                return false;
            }
            self.front = None;
        }
        // The band covers `growth` new rows plus the `band - growth` rows above them, so the ring
        // only has to hold the latter before it grows. Everything the growth pushes out of the
        // ring is more than a viewport above the bottom, which is what makes it final.
        if self.back.rows + growth > self.view {
            let excess = self.back.rows + growth - self.view;
            if !self.seal_ring(false, excess) {
                return false;
            }
        }
        let existing = band - growth;
        if self.back.rows < existing {
            return self.fail(format!(
                "a {band}-row band needs {existing} rows already held, and the ring has {}",
                self.back.rows
            ));
        }
        if !self.back.grow_back(growth) {
            return self.fail("the back edge buffer cannot take the growth".into());
        }
        self.rows += growth;
        self.back.write(self.back.rows - band, band_src);
        true
    }

    fn prepend(&mut self, growth: u32, band_src: &[u8]) -> bool {
        if self.rows + growth > self.max_rows {
            return false;
        }
        let band = (band_src.len() / self.row) as u32;
        debug_assert!(band <= self.view && band >= growth, "{band} {growth}");
        // The band covers the `growth` new top rows plus the `band - growth` rows that were already
        // the top, so those have to be unsealed from the middle before the prepend can write.
        let existing = band - growth;
        if !self.unseal_front(existing.max(1)) {
            return false;
        }
        if self.front_rows() + growth > self.view {
            let excess = self.front_rows() + growth - self.view;
            if !self.seal_ring(true, excess) {
                return false;
            }
        }
        if self.front_rows() < existing {
            return self.fail(format!(
                "a {band}-row band needs {existing} rows already held, and the front ring has {}",
                self.front_rows()
            ));
        }
        let front = self.front.as_mut().expect("unseal_front ensures a ring");
        if !front.grow_front(growth) {
            return self.fail("the front edge buffer cannot take the growth".into());
        }
        self.rows += growth;
        front.write(0, band_src);
        true
    }

    fn read_rows(&self, first_row: u32, rows: u32, out: &mut [u8]) {
        debug_assert_eq!(out.len(), rows as usize * self.row);
        debug_assert!(first_row + rows <= self.rows);
        self.gather(first_row, rows, out);
    }

    fn take_all(&mut self) -> Vec<u8> {
        let mut out = vec![0u8; self.rows as usize * self.row];
        let rows = self.rows;
        self.gather(0, rows, &mut out);
        out
    }

    fn budgets(&self) -> Budgets {
        let front = self.front.as_ref().map_or(0, |front| front.pixels.len() as u64);
        let back = self.back.pixels.len() as u64;
        Budgets {
            pixel_rows: self.rows,
            pixel_bytes: u64::from(self.rows) * self.row as u64,
            working_bytes: front + back,
            temp_bytes: self.middle_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BackedCanvas;
    use crate::scroll::store::{CanvasStore, MemoryCanvas};

    const W: u32 = 8;
    const VIEW: u32 = 200;

    fn frame(value: u8) -> Vec<u8> {
        vec![value; (W * VIEW * 4) as usize]
    }

    fn band_src(value: u8, band: u32) -> Vec<u8> {
        vec![value; (W * band * 4) as usize]
    }

    /// Every append must leave both stores holding the same bytes, because the backing store only
    /// decides *where* rows live, never what they are (docs/35 §11.2).
    #[test]
    fn appends_land_in_the_same_place_as_one_buffer() {
        let first = frame(1);
        let mut memory = MemoryCanvas::new(&first, W, VIEW, 64 * VIEW);
        let mut backed = BackedCanvas::new(&first, W, VIEW, 64 * VIEW);
        let mut value = 2u8;
        for step in 0..8u32 {
            let growth = 90;
            let band = (VIEW / 2).max(growth + VIEW / 4).min(VIEW);
            let src = band_src(value, band);
            assert!(memory.append(growth, &src), "memory step {step}");
            assert!(backed.append(growth, &src), "backed step {step}");
            assert_eq!(backed.rows(), memory.rows(), "rows after step {step}");
            let rows = memory.rows();
            let mut a = vec![0u8; (W * rows * 4) as usize];
            let mut b = vec![0u8; (W * rows * 4) as usize];
            memory.read_rows(0, rows, &mut a);
            backed.read_rows(0, rows, &mut b);
            if a != b {
                let row = (W * 4) as usize;
                let bad = (0..rows as usize).find(|r| a[r * row..(r + 1) * row] != b[r * row..(r + 1) * row]);
                panic!(
                    "step {step}: first differing row {:?} of {rows} (memory {:?}, backed {:?})",
                    bad,
                    bad.map(|r| a[r * row]),
                    bad.map(|r| b[r * row])
                );
            }
            value += 1;
        }
    }
}

#[cfg(test)]
mod budget_tests {
    use super::BackedCanvas;
    use crate::scroll::store::CanvasStore;

    const W: u32 = 4;
    const VIEW: u32 = 3;

    fn frame(value: u8) -> Vec<u8> {
        vec![value; (W * VIEW * 4) as usize]
    }

    /// A downward session holds one edge buffer and grows a tile file; the budgets say which is
    /// which, because they are not interchangeable (docs/35 §5.1).
    #[test]
    fn the_budgets_separate_working_memory_from_temporary_storage() {
        let mut store = BackedCanvas::new(&frame(1), W, VIEW, 16 * VIEW);
        let start = store.budgets();
        assert_eq!(start.pixel_rows, VIEW);
        assert_eq!(start.temp_bytes, 0, "a short session writes nothing");
        assert!(start.working_bytes >= start.pixel_bytes, "{start:?}");

        let row = (W * 4) as usize;
        let band = vec![2u8; 2 * row];
        assert!(store.append(2, &band));
        let grown = store.budgets();
        assert_eq!(grown.pixel_rows, VIEW + 2);
        assert_eq!(grown.working_bytes, start.working_bytes, "the ring is fixed");
        // The ring only ever holds one viewport, so this growth pushes two rows out of it and into
        // the tile file: the image grew by two rows and memory did not.
        assert_eq!(grown.temp_bytes, 2 * row as u64, "{grown:?}");
    }

    /// The tile file is the store's own: dropping the store removes it, so a cancelled or finished
    /// session leaves no working files behind (docs/35 §7.2).
    #[test]
    fn dropping_the_store_removes_its_tile_file() {
        fn mine() -> Vec<String> {
            std::fs::read_dir(std::env::temp_dir())
                .expect("temp dir")
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(&format!("snapclip-scroll-{}-", std::process::id())))
                .collect()
        }

        let mut store = BackedCanvas::new(&frame(1), W, VIEW, 64 * VIEW);
        let row = (W * 4) as usize;
        for step in 0..6u32 {
            let band = vec![step as u8 + 2; 2 * row];
            assert!(store.append(2, &band), "step {step}");
        }
        assert!(store.budgets().temp_bytes > 0, "rows left the ring: {:?}", store.budgets());
        assert!(!mine().is_empty(), "the tile file should exist while the store lives");
        drop(store);
        assert!(mine().is_empty(), "the tile file outlived its store: {:?}", mine());
    }
}
