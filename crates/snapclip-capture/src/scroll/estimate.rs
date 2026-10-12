//! How far the content moved between two frames of the same viewport.
//!
//! The facts this is derived from (`docs/33`):
//!
//! * a live page is almost never byte-identical from one frame to the next (cursor, caret,
//!   animation), so the answer is computed for **every** frame and never waits for stillness;
//! * scrolling translates a sub-region vertically by whole physical pixels while headers, footers,
//!   sidebars and scrollbars stay put;
//! * repeated content (blank rows, identical lines) cannot be located from content alone.
//!
//! Hence: the frame is cut into [`STRIPS`] column strips and every (strip, row) gets a hash. A hash
//! that occurs once in a strip of the reference frame is an anchor; a matching hash in the current
//! frame votes for `dy = y_ref - y_cur`. Splitting by columns keeps a fixed sidebar from spoiling
//! the rows of the content beside it, and hashes that repeat are dropped instead of guessed at.
//! The work is O(W·H) per frame.

use std::collections::HashMap;

/// Column strips per frame.
pub(crate) const STRIPS: usize = 8;

/// The fewest rows two frames must share for a shift to be measurable at all.
pub(crate) const MIN_OVERLAP: u32 = 32;

/// Absolute floor of votes for a non-zero shift.
const MIN_VOTES: u32 = 8;

/// Hashes of every (strip, row) of one frame.
#[derive(Debug, Clone)]
pub(crate) struct FrameSig {
    height: u32,
    /// `hashes[strip * height + row]`.
    hashes: Vec<u64>,
}

impl FrameSig {
    /// `bgra` is tightly packed, `width * height * 4` bytes.
    pub(crate) fn of(bgra: &[u8], width: u32, height: u32) -> Self {
        let (w, h) = (width as usize, height as usize);
        debug_assert_eq!(bgra.len(), w * h * 4);
        let bounds = strip_bounds(w);
        let mut hashes = vec![0u64; STRIPS * h];
        for y in 0..h {
            let row = &bgra[y * w * 4..(y + 1) * w * 4];
            for (strip, &(from, to)) in bounds.iter().enumerate() {
                hashes[strip * h + y] = hash_bytes(&row[from * 4..to * 4]);
            }
        }
        Self { height, hashes }
    }

    fn strip(&self, strip: usize) -> &[u64] {
        let h = self.height as usize;
        &self.hashes[strip * h..(strip + 1) * h]
    }
}

/// What two frames say about the motion between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Estimate {
    /// The content moved by `dy` rows: positive means it moved **up** (the page scrolled down), so
    /// the current frame's row `y` shows what the reference frame had at row `y + dy`.
    Shift { dy: i32, votes: u32, second: u32 },
    /// Nothing moved: the rows that stayed put outnumber the rows that are new.
    Still { votes: u32, unmatched: u32 },
    /// The evidence does not single out an answer.
    Unknown {
        best: i32,
        votes: u32,
        second: u32,
        unmatched: u32,
    },
}

/// Compare the current frame against the reference frame.
pub(crate) fn estimate(reference: &FrameSig, current: &FrameSig) -> Estimate {
    debug_assert_eq!(reference.height, current.height);
    let h = reference.height as i32;
    let max_shift = h - MIN_OVERLAP as i32;
    // dy in [-max_shift, max_shift] maps to index dy + max_shift.
    let span = (2 * max_shift.max(0) + 1) as usize;
    let mut votes = vec![0u32; span];
    let mut anchors = 0u32;
    // Rows of the current frame that are unique there and absent from the reference: content
    // that is new (revealed by a scroll, or replaced by a jump or an animation).
    let mut unmatched = 0u32;

    let mut unique: HashMap<u64, i32> = HashMap::with_capacity(h as usize);
    for strip in 0..STRIPS {
        unique.clear();
        for (y, &hash) in reference.strip(strip).iter().enumerate() {
            unique
                .entry(hash)
                .and_modify(|at| *at = -1)
                .or_insert(y as i32);
        }
        // A hash repeated in the *current* frame is just as ambiguous as one repeated in the
        // reference, so it is counted once and dropped if seen again.
        let mut seen: HashMap<u64, u32> = HashMap::with_capacity(h as usize);
        for &hash in current.strip(strip) {
            *seen.entry(hash).or_insert(0) += 1;
        }
        for (y, &hash) in current.strip(strip).iter().enumerate() {
            if seen[&hash] != 1 {
                continue;
            }
            let Some(&at) = unique.get(&hash) else {
                unmatched += 1;
                continue;
            };
            if at < 0 {
                continue;
            }
            anchors += 1;
            let dy = at - y as i32;
            if dy.abs() <= max_shift {
                votes[(dy + max_shift) as usize] += 1;
            }
        }
    }

    let still = if max_shift >= 0 {
        votes[max_shift as usize]
    } else {
        0
    };
    let mut best = (0i32, 0u32);
    for (index, &count) in votes.iter().enumerate() {
        let dy = index as i32 - max_shift;
        if dy != 0 && count > best.1 {
            best = (dy, count);
        }
    }
    // The runner-up must be a different answer, not the best one blurred by a row.
    let mut second = 0u32;
    for (index, &count) in votes.iter().enumerate() {
        let dy = index as i32 - max_shift;
        if dy != 0 && (dy - best.0).abs() > 2 && count > second {
            second = count;
        }
    }

    let floor = MIN_VOTES.max(anchors / 50);
    if best.1 >= floor && best.1 >= 2 * second.max(1) {
        return Estimate::Shift {
            dy: best.0,
            votes: best.1,
            second,
        };
    }
    // Fixed rows (a header, a footer, a sidebar) always vote for 0, so 0 winning is not enough:
    // when the content beside them is all new, the page jumped or was replaced, and adopting this
    // frame as the reference would lose the position.
    if still > 0 && still >= 2 * best.1 && still >= unmatched {
        return Estimate::Still {
            votes: still,
            unmatched,
        };
    }
    Estimate::Unknown {
        best: best.0,
        votes: best.1,
        second,
        unmatched,
    }
}

fn strip_bounds(width: usize) -> [(usize, usize); STRIPS] {
    let mut bounds = [(0, 0); STRIPS];
    for (strip, bound) in bounds.iter_mut().enumerate() {
        *bound = (width * strip / STRIPS, width * (strip + 1) / STRIPS);
    }
    bounds
}

/// 64-bit multiply-xor hash over 8-byte words; the tail is folded in byte by byte.
fn hash_bytes(bytes: &[u8]) -> u64 {
    const K: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut hash = 0xCBF2_9CE4_8422_2325u64 ^ bytes.len() as u64;
    let (words, tail) = bytes.as_chunks::<8>();
    for word in words {
        hash = (hash ^ u64::from_le_bytes(*word))
            .wrapping_mul(K)
            .rotate_left(29);
    }
    for &byte in tail {
        hash = (hash ^ u64::from(byte)).wrapping_mul(K);
    }
    hash ^ (hash >> 32)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{Estimate, FrameSig, estimate};

    /// A deterministic "page": every row differs from every other in every strip.
    pub(crate) fn page(width: u32, height: u32, seed: u32) -> Vec<u8> {
        let mut bytes = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let v = (y.wrapping_add(seed))
                    .wrapping_mul(2_654_435_761)
                    .wrapping_add(x.wrapping_mul(40_503));
                let v = v ^ (v >> 13);
                bytes.extend_from_slice(&[v as u8, (v >> 8) as u8, (v >> 16) as u8, 255]);
            }
        }
        bytes
    }

    /// Rows `[top, top + height)` of a page.
    pub(crate) fn window(page: &[u8], width: u32, top: u32, height: u32) -> Vec<u8> {
        let row = (width * 4) as usize;
        page[top as usize * row..(top + height) as usize * row].to_vec()
    }

    fn fill_rows(frame: &mut [u8], width: u32, from: u32, to: u32, color: [u8; 4]) {
        let row = (width * 4) as usize;
        for pixel in frame[from as usize * row..to as usize * row]
            .as_chunks_mut::<4>()
            .0
        {
            *pixel = color;
        }
    }

    fn paint(frame: &mut [u8], width: u32, x0: u32, y0: u32, x1: u32, y1: u32, seed: u8) {
        for y in y0..y1 {
            for x in x0..x1 {
                let at = ((y * width + x) * 4) as usize;
                frame[at] = seed.wrapping_add((x ^ y) as u8);
                frame[at + 1] = seed;
                frame[at + 2] = (x as u8).wrapping_mul(3);
                frame[at + 3] = 255;
            }
        }
    }

    fn sig(frame: &[u8], width: u32, height: u32) -> FrameSig {
        FrameSig::of(frame, width, height)
    }

    pub(crate) fn shift_of(estimate: Estimate) -> Option<i32> {
        match estimate {
            Estimate::Shift { dy, .. } => Some(dy),
            _ => None,
        }
    }

    const W: u32 = 160;
    const H: u32 = 200;

    #[test]
    fn a_plain_scroll_down_is_measured_exactly() {
        let page = page(W, 2000, 1);
        let a = window(&page, W, 100, H);
        for dy in [1, 7, 40, 120, (H - 32) as i32] {
            let b = window(&page, W, 100 + dy as u32, H);
            assert_eq!(shift_of(estimate(&sig(&a, W, H), &sig(&b, W, H))), Some(dy));
        }
    }

    #[test]
    fn a_scroll_up_is_measured_as_a_negative_shift() {
        let page = page(W, 2000, 2);
        let a = window(&page, W, 500, H);
        let b = window(&page, W, 437, H);
        assert_eq!(
            shift_of(estimate(&sig(&a, W, H), &sig(&b, W, H))),
            Some(-63)
        );
    }

    #[test]
    fn a_fixed_header_and_footer_do_not_pull_the_answer_to_zero() {
        let page = page(W, 2000, 3);
        let frame = |top: u32| {
            let mut frame = window(&page, W, top, H);
            paint(&mut frame, W, 0, 0, W, 40, 9); // header
            paint(&mut frame, W, 0, H - 30, W, H, 77); // footer
            frame
        };
        let (a, b) = (frame(300), frame(345));
        assert_eq!(shift_of(estimate(&sig(&a, W, H), &sig(&b, W, H))), Some(45));
    }

    #[test]
    fn a_fixed_sidebar_does_not_spoil_the_strips_beside_it() {
        let page = page(W, 2000, 4);
        let frame = |top: u32| {
            let mut frame = window(&page, W, top, H);
            // The left quarter is a sidebar that never moves.
            paint(&mut frame, W, 0, 0, W / 4, H, 31);
            frame
        };
        let (a, b) = (frame(200), frame(260));
        assert_eq!(shift_of(estimate(&sig(&a, W, H), &sig(&b, W, H))), Some(60));
    }

    #[test]
    fn a_blinking_caret_and_an_animation_do_not_hide_the_shift() {
        let page = page(W, 2000, 5);
        let a = window(&page, W, 100, H);
        let mut b = window(&page, W, 130, H);
        // A caret and a small animated block change between the two frames.
        paint(&mut b, W, 50, 60, 52, 80, 200);
        paint(&mut b, W, 100, 120, 140, 150, 99);
        assert_eq!(shift_of(estimate(&sig(&a, W, H), &sig(&b, W, H))), Some(30));
    }

    #[test]
    fn a_live_but_unscrolled_page_is_still() {
        let page = page(W, 2000, 6);
        let a = window(&page, W, 100, H);
        let mut b = a.clone();
        paint(&mut b, W, 10, 10, 30, 40, 123);
        assert!(matches!(
            estimate(&sig(&a, W, H), &sig(&b, W, H)),
            Estimate::Still { .. }
        ));
    }

    #[test]
    fn repeated_content_is_unknown_rather_than_guessed() {
        // Every row is one of two patterns: nothing is unique, so nothing can vote.
        let mut a = vec![0u8; (W * H * 4) as usize];
        for y in 0..H {
            fill_rows(
                &mut a,
                W,
                y,
                y + 1,
                if y % 2 == 0 {
                    [255, 255, 255, 255]
                } else {
                    [0, 0, 0, 255]
                },
            );
        }
        let mut b = a.clone();
        b.rotate_left((W * 4 * 2) as usize);
        assert!(matches!(
            estimate(&sig(&a, W, H), &sig(&b, W, H)),
            Estimate::Unknown { .. }
        ));
    }

    #[test]
    fn a_jump_beyond_the_overlap_floor_is_unknown() {
        let page = page(W, 2000, 7);
        let a = window(&page, W, 100, H);
        let b = window(&page, W, 100 + H - 10, H);
        assert!(matches!(
            estimate(&sig(&a, W, H), &sig(&b, W, H)),
            Estimate::Unknown { .. }
        ));
    }

    /// A jump past the overlap under a fixed header and footer: the only rows the two frames share
    /// are the fixed ones, and they all vote for 0. That is not "nothing moved" — the content beside
    /// them is new — and calling it still would adopt the new frame as the reference and lose the
    /// position, leaving a silent gap in the image.
    #[test]
    fn a_jump_under_a_fixed_header_is_unknown_not_still() {
        let page = page(W, 4000, 10);
        let frame = |top: u32| {
            let mut frame = window(&page, W, top, H);
            paint(&mut frame, W, 0, 0, W, 40, 9); // header
            paint(&mut frame, W, 0, H - 30, W, H, 77); // footer
            frame
        };
        let (a, b) = (frame(100), frame(100 + 2 * H));
        assert!(matches!(
            estimate(&sig(&a, W, H), &sig(&b, W, H)),
            Estimate::Unknown { .. }
        ));
    }

    /// `W × H` of a page zoomed by `scale` about its top-left corner, starting at page row `top`
    /// (nearest-neighbour, the way a zoom changes which page pixel lands on each screen pixel).
    fn zoomed(page: &[u8], page_width: u32, top: u32, scale: f64) -> Vec<u8> {
        let mut frame = Vec::with_capacity((W * H * 4) as usize);
        for y in 0..H {
            let source_y = top + (f64::from(y) / scale) as u32;
            for x in 0..W {
                let source_x = (f64::from(x) / scale) as u32;
                let at = ((source_y * page_width + source_x) * 4) as usize;
                frame.extend_from_slice(&page[at..at + 4]);
            }
        }
        frame
    }

    /// Ctrl+wheel during a session zooms the page instead of scrolling it. Stitching a zoomed frame
    /// would splice two scales into one image, so it must be `Unknown` (the reference is kept, and
    /// zooming back recovers). The striped page is the hard case: every row is one solid colour,
    /// so a zoomed frame's rows still match page rows exactly — but at a dy that changes from row to
    /// row, which no single shift explains.
    #[test]
    fn a_zoomed_frame_is_unknown_not_a_shift() {
        let page_width = 2 * W;
        let textured = page(page_width, 2000, 11);
        let mut striped = vec![0u8; (page_width * 2000 * 4) as usize];
        for (y, row) in striped
            .chunks_exact_mut((page_width * 4) as usize)
            .enumerate()
        {
            let v = (y as u32).wrapping_mul(2_654_435_761);
            for pixel in row.as_chunks_mut::<4>().0 {
                *pixel = [v as u8, (v >> 8) as u8, (v >> 16) as u8, 255];
            }
        }
        for (name, page) in [("textured", &textured), ("striped", &striped)] {
            let reference = sig(&zoomed(page, page_width, 300, 1.0), W, H);
            for scale in [0.8, 1.25, 1.5, 2.0] {
                let result = estimate(
                    &reference,
                    &sig(&zoomed(page, page_width, 300, scale), W, H),
                );
                assert!(
                    matches!(result, Estimate::Unknown { .. }),
                    "{name} page at {scale}x: {result:?}"
                );
            }
        }
    }

    #[test]
    fn a_different_page_is_unknown() {
        let a = window(&page(W, 2000, 8), W, 100, H);
        let b = window(&page(W, 2000, 999), W, 100, H);
        assert!(matches!(
            estimate(&sig(&a, W, H), &sig(&b, W, H)),
            Estimate::Unknown { .. }
        ));
    }

    /// Cost evidence for the plan's "O(W·H) per frame": the selection of the second field log.
    /// Reported, not asserted — debug builds and loaded machines vary too much for a threshold.
    #[test]
    fn the_cost_of_one_estimate_at_the_field_selection_size() {
        let (w, h) = (1241u32, 821u32);
        let page = page(w, 3000, 9);
        let a = window(&page, w, 0, h);
        let b = window(&page, w, 90, h);
        let started = std::time::Instant::now();
        let (sa, sb) = (sig(&a, w, h), sig(&b, w, h));
        let result = estimate(&sa, &sb);
        eprintln!(
            "estimate {w}x{h}: {} us (two signatures + vote)",
            started.elapsed().as_micros()
        );
        assert_eq!(shift_of(result), Some(90));
    }
}
