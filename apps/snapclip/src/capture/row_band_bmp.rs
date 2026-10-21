//! The BMP row sink (`docs/49`).
//!
//! BMP is the one format whose natural writing order is ours: the header is a fixed size, a
//! negative `biHeight` stores rows top-down, and 32-bit `BI_RGB` is already the BGRA the capture
//! path hands over, so a row is a copy and nothing else.
//!
//! Alpha is not preserved: the fourth byte of a 32-bit `BI_RGB` row is reserved, and every viewer
//! this product writes for treats it that way. Screenshots are opaque.

use std::io::Write;

use super::row_band_png::{ExportError, ImageMeta};

pub const BMP_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

const PIXEL_OFFSET: u64 = 14 + 124;

pub struct BmpRowBandWriter {
    width: u64,
    height: u64,
    written: u64,
    file_bytes: u64,
    header_written: bool,
}

impl BmpRowBandWriter {
    pub fn begin(meta: &ImageMeta) -> Result<Self, ExportError> {
        if meta.width == 0 || meta.height == 0 {
            return Err(ExportError::TooLarge { width: meta.width, height: meta.height });
        }
        let pixels = meta.width.saturating_mul(meta.height);
        let file_bytes = PIXEL_OFFSET + pixels.saturating_mul(4);
        if file_bytes > BMP_MAX_FILE_BYTES {
            return Err(ExportError::TooLarge { width: meta.width, height: meta.height });
        }
        Ok(Self { width: meta.width, height: meta.height, written: 0, file_bytes, header_written: false })
    }

    fn header(&self) -> [u8; PIXEL_OFFSET as usize] {
        let mut header = [0u8; PIXEL_OFFSET as usize];
        let mut at = 0;
        let mut put = |bytes: &[u8]| {
            header[at..at + bytes.len()].copy_from_slice(bytes);
            at += bytes.len();
        };
        put(b"BM");
        put(&(self.file_bytes as u32).to_le_bytes());
        put(&0u16.to_le_bytes());
        put(&0u16.to_le_bytes());
        put(&(PIXEL_OFFSET as u32).to_le_bytes());
        put(&124u32.to_le_bytes());
        put(&(self.width as i32).to_le_bytes());
        put(&(-(self.height as i32)).to_le_bytes());
        put(&1u16.to_le_bytes());
        put(&32u16.to_le_bytes());
        put(&0u32.to_le_bytes());
        put(&((self.width * self.height * 4) as u32).to_le_bytes());
        put(&0i32.to_le_bytes());
        put(&0i32.to_le_bytes());
        put(&0u32.to_le_bytes());
        put(&0u32.to_le_bytes());
        put(&0x00FF_0000u32.to_le_bytes());
        put(&0x0000_FF00u32.to_le_bytes());
        put(&0x0000_00FFu32.to_le_bytes());
        put(&0xFF00_0000u32.to_le_bytes());
        put(&0u32.to_le_bytes());
        put(&[0u8; 36]);
        put(&[0u8; 12]);
        put(&0u32.to_le_bytes());
        put(&0u32.to_le_bytes());
        put(&0u32.to_le_bytes());
        put(&0u32.to_le_bytes());
        debug_assert_eq!(at, PIXEL_OFFSET as usize);
        header
    }

    pub fn write_rows(&mut self, first_row: u64, rows: &[u8], out: &mut dyn Write) -> Result<(), ExportError> {
        if first_row != self.written {
            return Err(ExportError::OutOfOrder { first_row, expected: self.written });
        }
        let row_bytes = self.width as usize * 4;
        if rows.is_empty() || rows.len() % row_bytes != 0 {
            return Err(ExportError::RowLength { expected: row_bytes as u64, got: rows.len() as u64 });
        }
        let count = (rows.len() / row_bytes) as u64;
        if self.written + count > self.height {
            return Err(ExportError::BeyondHeight { first_row, height: self.height });
        }
        if !self.header_written {
            out.write_all(&self.header())
                .map_err(|error| ExportError::Sink(format!("the BMP header cannot be written: {error}")))?;
            self.header_written = true;
        }
        out.write_all(rows)
            .map_err(|error| ExportError::Sink(format!("the BMP rows cannot be written: {error}")))?;
        self.written += count;
        Ok(())
    }

    pub fn finish(mut self, out: &mut dyn Write) -> Result<u64, ExportError> {
        if !self.header_written {
            out.write_all(&self.header())
                .map_err(|error| ExportError::Sink(format!("the BMP header cannot be written: {error}")))?;
            self.header_written = true;
        }
        if self.written != self.height {
            return Err(ExportError::Sink(format!("the BMP has {} of {} rows", self.written, self.height)));
        }
        Ok(self.file_bytes)
    }
}


