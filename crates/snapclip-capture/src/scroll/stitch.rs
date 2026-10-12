//! The long image: frames placed at the content position the estimator measured.
//!
//! The canvas is one contiguous BGRA buffer, `width` pixels wide. `pos` is the canvas row the top
//! of the viewport currently shows. A measured shift moves `pos`; when the viewport leaves the
//! canvas, the canvas grows by exactly the rows that were never seen before.
//!
//! Growth rewrites a **band** at the growing edge rather than only the new rows: a fixed footer
//! (or, scrolling up, a fixed header) sits at that edge of every frame, and rewriting at least
//! `growth + H/4` rows from the newest frame is what replaces the previous copy of it with real
//! content. Only the last copy survives, where it belongs. The opposite edge is never written, so
//! a fixed bar on that side appears once. The band is the reference's choice (`snow_shot`'s
//! `compositor.rs`): `max(H/2, growth + H/4)`, at most `H`.

/// What one measured shift did to the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    /// The canvas grew at the bottom by `growth` rows; its last `band` rows were rewritten.
    Appended { growth: u32, band: u32 },
    /// The canvas grew at the top by `growth` rows; its first `band` rows were rewritten.
    Prepended { growth: u32, band: u32 },
    /// The viewport moved inside rows that already exist.
    Contained,
    /// Growing would pass the row cap, or the memory for it could not be had; nothing changed.
    Full,
}

#[derive(Debug)]
pub(crate) struct Stitcher {
    width: u32,
    view: u32,
    max_rows: u32,
    canvas: Vec<u8>,
    rows: u32,
    pos: u32,
    /// Content coordinate of canvas row 0: the first frame's top is 0, and every prepend lowers it.
    top: i64,
}

impl Stitcher {
    /// Start from the first frame. `max_rows` caps the canvas height.
    pub(crate) fn new(first: &[u8], width: u32, view: u32, max_rows: u32) -> Self {
        debug_assert_eq!(first.len(), (width * view * 4) as usize);
        Self {
            width,
            view,
            max_rows: max_rows.max(view),
            canvas: first.to_vec(),
            rows: view,
            pos: 0,
            top: 0,
        }
    }

    pub(crate) fn rows(&self) -> u32 {
        self.rows
    }

    /// The canvas row the viewport's top shows.
    pub(crate) fn pos(&self) -> u32 {
        self.pos
    }

    /// Content coordinate of canvas row 0.
    pub(crate) fn top(&self) -> i64 {
        self.top
    }

    pub(crate) fn canvas(&self) -> &[u8] {
        &self.canvas
    }

    pub(crate) fn into_canvas(self) -> Vec<u8> {
        self.canvas
    }

    /// Place `frame`, whose content moved by `dy` relative to the frame placed last
    /// (positive: the page scrolled down).
    pub(crate) fn place(&mut self, frame: &[u8], dy: i32) -> Placement {
        debug_assert_eq!(frame.len(), (self.width * self.view * 4) as usize);
        let row = (self.width * 4) as usize;
        let view = self.view as i64;
        let cand = i64::from(self.pos) + i64::from(dy);
        if cand + view > i64::from(self.rows) {
            let growth = (cand + view - i64::from(self.rows)) as u32;
            if self.rows + growth > self.max_rows
                || !grow(
                    &mut self.canvas,
                    growth as usize * row,
                    self.max_rows as usize * row,
                )
            {
                return Placement::Full;
            }
            self.rows += growth;
            let band = self.band(growth) as usize;
            let to = self.rows as usize * row;
            self.canvas[to - band * row..to]
                .copy_from_slice(&frame[(self.view as usize - band) * row..]);
            self.pos = self.rows - self.view;
            Placement::Appended {
                growth,
                band: band as u32,
            }
        } else if cand < 0 {
            let growth = (-cand) as u32;
            let known = self.canvas.len();
            if self.rows + growth > self.max_rows
                || !grow(
                    &mut self.canvas,
                    growth as usize * row,
                    self.max_rows as usize * row,
                )
            {
                return Placement::Full;
            }
            self.rows += growth;
            self.canvas.copy_within(..known, growth as usize * row);
            let band = self.band(growth) as usize;
            self.canvas[..band * row].copy_from_slice(&frame[..band * row]);
            self.pos = 0;
            self.top -= i64::from(growth);
            Placement::Prepended {
                growth,
                band: band as u32,
            }
        } else {
            self.pos = cand as u32;
            Placement::Contained
        }
    }

    fn band(&self, growth: u32) -> u32 {
        (self.view / 2).max(growth + self.view / 4).min(self.view)
    }
}

/// Append `bytes` zeroes to `canvas`, or leave it untouched and return `false` when the memory
/// cannot be had. A long image is the one allocation here that the user's scrolling sizes, and
/// running out must end the growth, not the process (`Vec::resize` aborts on failure).
///
/// Capacity doubles, so a session moves the canvas O(log n) times rather than once per append
/// (each move copies the whole image). `limit` caps only that doubling choice: the call is bounded
/// by `len + bytes` when that is already larger, so this function never refuses a growth the
/// caller's own row cap allowed. The row cap ([`super::row_limit`]) is what keeps the canvas inside
/// the byte budget; this is the last line against an allocation the machine cannot serve.
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

#[cfg(test)]
mod tests {
    use super::{Placement, Stitcher, grow};
    use crate::scroll::estimate::tests::{page, window};
    use crate::scroll::estimate::{Estimate, FrameSig, estimate};

    const W: u32 = 96;
    const H: u32 = 160;
    const HEADER: u32 = 24;
    const FOOTER: u32 = 18;

    /// What the viewport shows at page offset `top`: the page, under a fixed header and footer.
    fn frame_at(page: &[u8], top: u32) -> Vec<u8> {
        let mut frame = window(page, W, top, H);
        let row = (W * 4) as usize;
        for y in 0..HEADER {
            for x in 0..W {
                let at = y as usize * row + x as usize * 4;
                frame[at..at + 4].copy_from_slice(&[x as u8, y as u8, 7, 255]);
            }
        }
        for y in H - FOOTER..H {
            for x in 0..W {
                let at = y as usize * row + x as usize * 4;
                frame[at..at + 4].copy_from_slice(&[y as u8, x as u8, 201, 255]);
            }
        }
        frame
    }

    /// The image a perfect stitch of offsets `0..=last` must produce: the header once, the page
    /// rows the viewport's content area ever showed, and the footer once at the bottom.
    fn truth(page: &[u8], last: u32) -> Vec<u8> {
        let row = (W * 4) as usize;
        let first = frame_at(page, 0);
        let final_frame = frame_at(page, last);
        let mut out = first[..HEADER as usize * row].to_vec();
        out.extend_from_slice(&page[HEADER as usize * row..(last + H - FOOTER) as usize * row]);
        out.extend_from_slice(&final_frame[(H - FOOTER) as usize * row..]);
        out
    }

    /// Drive estimator + stitcher over a sequence of page offsets, the way the worker does.
    fn run(page: &[u8], offsets: &[u32]) -> Stitcher {
        let first = frame_at(page, offsets[0]);
        let mut stitcher = Stitcher::new(&first, W, H, 32 * H);
        let mut reference = FrameSig::of(&first, W, H);
        for &top in &offsets[1..] {
            let frame = frame_at(page, top);
            let current = FrameSig::of(&frame, W, H);
            match estimate(&reference, &current) {
                Estimate::Shift { dy, .. } => {
                    stitcher.place(&frame, dy);
                    reference = current;
                }
                Estimate::Still { .. } => reference = current,
                Estimate::Unknown { .. } => {}
            }
        }
        stitcher
    }

    #[test]
    fn a_scroll_down_reproduces_the_page_with_one_header_and_one_footer() {
        let page = page(W, 4000, 11);
        // Uneven steps, including the small in-between offsets a smooth scroll produces.
        // Every step stays inside the content area's overlap, as consecutive frames do.
        let offsets = [
            0, 3, 40, 41, 97, 150, 151, 210, 283, 340, 400, 470, 471, 530,
        ];
        let stitcher = run(&page, &offsets);
        assert_eq!(stitcher.rows(), 530 + H);
        assert!(
            stitcher.canvas() == truth(&page, 530).as_slice(),
            "the stitched image must equal the page byte for byte"
        );
    }

    #[test]
    fn scrolling_back_up_and_down_again_changes_nothing_already_seen() {
        let page = page(W, 4000, 12);
        let offsets = [0, 60, 120, 70, 20, 90, 150, 210];
        let stitcher = run(&page, &offsets);
        assert_eq!(stitcher.rows(), 210 + H);
        assert_eq!(stitcher.pos(), 210);
        assert!(stitcher.canvas() == truth(&page, 210).as_slice());
    }

    #[test]
    fn a_jump_the_estimator_cannot_measure_is_skipped_and_the_session_recovers() {
        let page = page(W, 4000, 13);
        // 80 -> 280 leaves no overlap, so it is Unknown; the reference stays at 80, and the next
        // frame (160) is again measurable against it.
        let offsets = [0, 80, 280, 160, 230];
        let stitcher = run(&page, &offsets);
        assert_eq!(stitcher.rows(), 230 + H);
        assert!(stitcher.canvas() == truth(&page, 230).as_slice());
    }

    #[test]
    fn scrolling_up_from_the_middle_grows_the_canvas_at_the_top() {
        let page = page(W, 4000, 14);
        let first = window(&page, W, 1000, H);
        let mut stitcher = Stitcher::new(&first, W, H, 32 * H);
        let up = window(&page, W, 950, H);
        assert_eq!(
            stitcher.place(&up, -50),
            Placement::Prepended {
                growth: 50,
                band: 90
            }
        );
        assert_eq!(stitcher.pos(), 0);
        assert_eq!(stitcher.top(), -50);
        let row = (W * 4) as usize;
        assert!(stitcher.canvas() == &page[950 * row..(1000 + H as usize) * row]);
    }

    #[test]
    fn the_canvas_stops_growing_at_its_cap() {
        let page = page(W, 4000, 15);
        let first = window(&page, W, 0, H);
        let mut stitcher = Stitcher::new(&first, W, H, H + 100);
        assert_eq!(
            stitcher.place(&window(&page, W, 80, H), 80),
            Placement::Appended {
                growth: 80,
                band: 120
            }
        );
        assert_eq!(
            stitcher.place(&window(&page, W, 160, H), 80),
            Placement::Full
        );
        assert_eq!(stitcher.rows(), H + 80);
        assert_eq!(stitcher.pos(), 80);
    }

    /// Out of memory is the end of the image, not of the process: the canvas is left as it was.
    #[test]
    fn a_canvas_that_cannot_grow_is_left_untouched() {
        let mut canvas = vec![7u8; 64];
        assert!(!grow(&mut canvas, usize::MAX - 8, usize::MAX));
        assert_eq!(canvas, vec![7u8; 64]);
        assert!(grow(&mut canvas, 16, 96));
        assert!(canvas.capacity() <= 96, "doubling stops at the limit");
        assert_eq!(canvas.len(), 80);
        assert!(canvas[..64].iter().all(|&b| b == 7) && canvas[64..].iter().all(|&b| b == 0));
    }

    /// How much the canvas really moves over a field-sized session. The field log's rising append
    /// cost (68 ms at 1.4k rows, 123 ms at 42k) is not reallocation: at 1323x36000 the canvas moves
    /// six times over 390 appends, for 35.7 MB copied in total. So the per-append cost that does rise
    /// with the image is elsewhere — the prepend path shifts the whole canvas down by the growth
    /// band by construction (`copy_within`, stitch.rs:117). Reported, not asserted.
    #[test]
    #[ignore = "timing report: run with --release --ignored --nocapture"]
    fn the_canvas_moves_a_logarithmic_number_of_times_at_the_field_size() {
        const W: u32 = 1323;
        const H: u32 = 900;
        const MAX_ROWS: u32 = 40 * H;
        let page = page(W, MAX_ROWS, 5);
        let mut stitcher = Stitcher::new(&window(&page, W, 0, H), W, H, MAX_ROWS);
        let mut moves = 0u32;
        let mut moved = 0u64;
        let mut capacity = stitcher.canvas.capacity();
        let mut appends = 0u32;
        let mut top = 0;
        while top + 90 + H <= MAX_ROWS {
            top += 90;
            let before = stitcher.canvas.len();
            if !matches!(
                stitcher.place(&window(&page, W, top, H), 90),
                Placement::Appended { .. }
            ) {
                break;
            }
            appends += 1;
            let now = stitcher.canvas.capacity();
            if now != capacity {
                moves += 1;
                moved += before as u64;
                capacity = now;
            }
        }
        println!(
            "canvas {W}x{}: {appends} appends, {moves} moves, {:.1} MB copied, peak {:.1} MB",
            stitcher.rows(),
            moved as f64 / 1e6,
            capacity as f64 / 1e6,
        );
    }

    /// The field log: every frame cost more the longer the image got (68 ms at 1.4k rows, 123 ms
    /// at 42k). `grow` is what keeps that from being an allocation per notch: the canvas moves
    /// O(log n) times, not once per append, and never reserves past the cap. The append cost that
    /// still rises with the image is the prepend shift, not this.
    #[test]
    fn a_long_session_moves_the_canvas_a_logarithmic_number_of_times() {
        let page = page(W, 32 * H + H, 5);
        let mut stitcher = Stitcher::new(&window(&page, W, 0, H), W, H, 32 * H);
        let mut moves = 0;
        let mut capacity = stitcher.canvas.capacity();
        let mut top = 0;
        while top + 9 + H <= 32 * H {
            top += 9;
            assert!(matches!(
                stitcher.place(&window(&page, W, top, H), 9),
                Placement::Appended { .. }
            ));
            if stitcher.canvas.capacity() != capacity {
                moves += 1;
                capacity = stitcher.canvas.capacity();
            }
        }
        assert!(moves <= 8, "{moves} reallocations over {} appends", top / 9);
        assert!(stitcher.canvas.capacity() <= (32 * H * W * 4) as usize);
        assert_eq!(
            stitcher.canvas(),
            &page[..(stitcher.rows() * W * 4) as usize]
        );
    }

    /// Performance evidence at the field size (2148x1344, 90-row notches, up to the 32-screen cap):
    /// the cost of one append, early and late in a session. Reported, not asserted.
    #[test]
    #[ignore = "timing report: run with --release --ignored --nocapture"]
    fn the_cost_of_an_append_at_the_field_size() {
        let (width, view) = (2148u32, 1344u32);
        let frame = vec![0x5au8; (width * view * 4) as usize];
        let mut stitcher = Stitcher::new(&frame, width, view, 32 * view);
        let mut costs = Vec::new();
        while let Placement::Appended { .. } = {
            let started = std::time::Instant::now();
            let placed = stitcher.place(&frame, 90);
            costs.push(started.elapsed().as_micros());
            placed
        } {}
        let mean = |part: &[u128]| part.iter().sum::<u128>() / part.len() as u128;
        println!(
            "appends={} first50_mean_us={} last50_mean_us={} max_us={} rows={}",
            costs.len() - 1,
            mean(&costs[..50]),
            mean(&costs[costs.len() - 51..costs.len() - 1]),
            costs.iter().max().unwrap(),
            stitcher.rows()
        );
    }

    #[test]
    fn a_move_inside_known_rows_writes_nothing() {
        let page = page(W, 4000, 16);
        let first = window(&page, W, 0, H);
        let mut stitcher = Stitcher::new(&first, W, H, 32 * H);
        stitcher.place(&window(&page, W, 100, H), 100);
        let before = stitcher.canvas().to_vec();
        // A frame that differs from the page would show up if it were written.
        let garbage = vec![9u8; (W * H * 4) as usize];
        assert_eq!(stitcher.place(&garbage, -40), Placement::Contained);
        assert_eq!(stitcher.pos(), 60);
        assert!(stitcher.canvas() == before.as_slice());
    }
}
