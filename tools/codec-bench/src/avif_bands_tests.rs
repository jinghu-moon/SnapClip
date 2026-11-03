use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
struct Flag(Arc<AtomicBool>);
impl Stop for Flag {
    fn check(&self) -> Result<(), enough::StopReason> {
        if self.0.load(Ordering::Relaxed) {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

struct FaultyBands {
    inner: SyntheticBands,
    fault: &'static str,
    stop: Flag,
}
impl FaultyBands {
    fn new(fault: &'static str) -> Self {
        Self {
            inner: SyntheticBands::new(133, 257),
            fault,
            stop: Flag(Arc::new(AtomicBool::new(false))),
        }
    }
}
impl BandSource for FaultyBands {
    fn source(&self) -> PixelSource {
        let generation = if (self.fault == "generation" && self.inner.reads > 0)
            || (self.fault == "later_generation" && self.inner.reads > 1)
        {
            74
        } else {
            73
        };
        let descriptor = match self.fault {
            "sdr" => PixelDescriptor::bgra8(),
            "relative" => PixelDescriptor::new(
                PixelFormat::Rgba16Float,
                PixelEncoding::LinearSrgb,
                AlphaMode::Straight,
            )
            .unwrap(),
            "descriptor" if self.inner.reads > 0 => PixelDescriptor::new(
                PixelFormat::Rgba16Float,
                PixelEncoding::ScRgb,
                AlphaMode::Premultiplied,
            )
            .unwrap(),
            _ => source().descriptor(),
        };
        PixelSource::new(descriptor, generation)
    }
    fn rows(&self) -> u32 {
        self.inner.height - u32::from(self.fault == "height" && self.inner.reads > 0)
    }
    fn read_rows(&mut self, first: u32, count: u32, out: &mut Vec<u8>) -> bool {
        assert!(count as usize * self.inner.width as usize * 8 <= READ_BYTES);
        assert_eq!(first as usize, self.inner.reads * 128);
        assert!(self.inner.read_rows(first, count, out));
        match self.fault {
            "read" => return false,
            "short" => {
                out.pop();
            }
            "extra" => out.extend_from_slice(&[0; 8]),
            "nan" => out[..2].copy_from_slice(&0x7e00u16.to_le_bytes()),
            "alpha" => out[6..8].copy_from_slice(&0x4000u16.to_le_bytes()),
            "range" => out[..8].copy_from_slice(&[0x00, 0xbc, 0, 0, 0, 0, 0x00, 0x3c]),
            "peak" => out[..8].copy_from_slice(&[0x00, 0x58, 0x00, 0x58, 0x00, 0x58, 0x00, 0x3c]),
            "cancel" => self.stop.0.store(true, Ordering::Relaxed),
            "later_cancel" if self.inner.reads > 1 => self.stop.0.store(true, Ordering::Relaxed),
            _ => (),
        }
        true
    }
}

#[test]
fn source_and_durable_master_agree_with_pq_conversion_and_preserve_original_bits() {
    let mut bands = SyntheticBands::new(133, 257);
    let bytes = encode(&mut bands, 133, StopToken::new(Unstoppable)).unwrap();
    assert_eq!(bands.reads, 3);
    assert!(bands.largest_read <= READ_BYTES);
    let palette = palette();
    let mut seen = vec![false; 133 * 257];
    let mut max_error = 0u16;
    let info = super::super::avif_grid::decode(&bytes, &Unstoppable, |left, top, pixels| {
        let tile = pixels.try_as_imgref::<Rgba<u16>>().unwrap();
        for (ix, p) in tile.pixels().enumerate() {
            let x = left as usize + ix % tile.width();
            let y = top as usize + ix / tile.width();
            assert!(!seen[y * 133 + x]);
            seen[y * 133 + x] = true;
            let expected = palette[fixture_key(x, y)];
            for (a, b) in [
                (p.r, expected.r),
                (p.g, expected.g),
                (p.b, expected.b),
                (p.a, expected.a),
            ] {
                max_error = max_error.max(a.abs_diff(b));
            }
        }
        Ok(())
    })
    .unwrap();
    assert!(seen.iter().all(|v| *v));
    assert!(f64::from(max_error) * 1023. / 65535. <= 2.);
    assert_eq!(
        (info.width, info.height, info.bit_depth, info.has_alpha),
        (133, 257, 10, true)
    );
    assert!(info.icc_profile.is_none());
    let directory = Directory::new().unwrap();
    let reference = write_master(&CaptureArtifactStore::new(&directory.0), &mut bands).unwrap();
    let original = std::fs::read(&reference.absolute_path).unwrap();
    let mut master = PixelMaster::open(&reference).unwrap();
    assert_eq!(master.metadata().source, source());
    assert_eq!(
        encode_master(&mut master, StopToken::new(Unstoppable)).unwrap(),
        bytes
    );
    let last = master.read_rows(256, 1).unwrap();
    let mut expected = Vec::new();
    assert!(bands.read_rows(256, 1, &mut expected));
    assert_eq!(last.as_bytes(), expected);
    assert_eq!(std::fs::read(&reference.absolute_path).unwrap(), original);
}

#[test]
fn changed_generation_descriptor_and_height_fail_on_the_first_source_read() {
    for fault in ["generation", "descriptor", "height"] {
        let mut bands = FaultyBands::new(fault);
        let error = encode(&mut bands, 133, StopToken::new(Unstoppable)).unwrap_err();
        assert!(error.contains("changed"), "{fault}: {error}");
        assert_eq!(bands.inner.reads, 1);
    }
}

#[test]
fn source_failure_bad_length_nonfinite_alpha_and_unrepresentable_colors_are_errors() {
    for fault in ["read", "short", "extra", "nan", "alpha", "range", "peak"] {
        let mut bands = FaultyBands::new(fault);
        let error = encode(&mut bands, 133, StopToken::new(Unstoppable)).unwrap_err();
        let expected = match fault {
            "read" => "refused",
            "short" | "extra" => "buffer length",
            "nan" => "non-finite",
            "alpha" => "alpha",
            _ => "Rec.2020/PQ range",
        };
        assert!(error.contains(expected), "{fault}: {error}");
        assert_eq!(bands.inner.reads, 1);
    }
}

#[test]
fn relative_sdr_and_oversized_geometry_fail_before_reading_pixels() {
    for fault in ["sdr", "relative"] {
        let mut bands = FaultyBands::new(fault);
        assert!(
            encode(&mut bands, 133, StopToken::new(Unstoppable))
                .unwrap_err()
                .contains("absolute scRGB")
        );
        assert_eq!(bands.inner.reads, 0);
    }
    for (width, height) in [
        (0, 1),
        (1, 0),
        (32640, 257),
        (133, 32641),
        (128, 128 * 4097),
    ] {
        let mut bands = SyntheticBands::new(width, height);
        assert!(encode(&mut bands, width, StopToken::new(Unstoppable)).is_err());
        assert_eq!(bands.reads, 0);
    }
}

#[test]
fn cancellation_before_and_after_source_read_returns_no_partial_grid() {
    let mut bands = FaultyBands::new("cancel");
    bands.stop.0.store(true, Ordering::Relaxed);
    let token = StopToken::new(bands.stop.clone());
    assert!(
        encode(&mut bands, 133, token)
            .unwrap_err()
            .to_lowercase()
            .contains("cancel")
    );
    assert_eq!(bands.inner.reads, 0);
    bands.stop.0.store(false, Ordering::Relaxed);
    let token = StopToken::new(bands.stop.clone());
    assert!(
        encode(&mut bands, 133, token)
            .unwrap_err()
            .to_lowercase()
            .contains("cancel")
    );
    assert_eq!(bands.inner.reads, 1);
}

#[test]
fn failure_after_the_first_encoded_tile_row_discards_partial_work() {
    for fault in ["later_generation", "later_cancel"] {
        let mut bands = FaultyBands::new(fault);
        let token = StopToken::new(bands.stop.clone());
        let error = encode(&mut bands, 133, token).unwrap_err();
        assert_eq!(bands.inner.reads, 2);
        assert!(error.to_lowercase().contains(if fault == "later_cancel" {
            "cancel"
        } else {
            "changed"
        }));
    }
}

#[test]
fn premultiplied_input_is_unassociated_once_and_zero_alpha_remains_finite() {
    struct Constant {
        bits: [u16; 4],
        alpha: AlphaMode,
    }
    impl BandSource for Constant {
        fn source(&self) -> PixelSource {
            PixelSource::new(
                PixelDescriptor::new(PixelFormat::Rgba16Float, PixelEncoding::ScRgb, self.alpha)
                    .unwrap(),
                73,
            )
        }
        fn rows(&self) -> u32 {
            2
        }
        fn read_rows(&mut self, _: u32, count: u32, out: &mut Vec<u8>) -> bool {
            *out = (0..count * 2)
                .flat_map(|ix| if ix % 2 == 0 { self.bits } else { [0; 4] })
                .flat_map(u16::to_le_bytes)
                .collect();
            true
        }
    }
    let mut premultiplied = Constant {
        bits: [0x3c00, 0x4000, 0x3800, 0x3800],
        alpha: AlphaMode::Premultiplied,
    };
    let mut straight = Constant {
        bits: [0x4000, 0x4400, 0x3c00, 0x3800],
        alpha: AlphaMode::Straight,
    };
    let a = encode(&mut premultiplied, 2, StopToken::new(Unstoppable)).unwrap();
    let b = encode(&mut straight, 2, StopToken::new(Unstoppable)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn invalid_tile_provider_length_and_error_do_not_return_a_container() {
    for fail in [false, true] {
        let mut calls = 0;
        let error = super::super::avif_grid::encode_tiles(
            128,
            128,
            128,
            None,
            StopToken::new(Unstoppable),
            MAX_PAYLOAD,
            |_, _, _| {
                calls += 1;
                if fail {
                    Err("source failure".into())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(calls, 1);
        assert!(error.contains(if fail { "source failure" } else { "incomplete" }));
    }
}

#[test]
fn a_damaged_master_is_refused_instead_of_replacing_it_with_a_proxy() {
    let directory = Directory::new().unwrap();
    let mut bands = SyntheticBands::new(133, 257);
    let reference = write_master(&CaptureArtifactStore::new(&directory.0), &mut bands).unwrap();
    let mut master = PixelMaster::open(&reference).unwrap();
    // Corruption after open must still be caught by the touched tile's checksum.
    use std::io::{Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&reference.absolute_path)
        .unwrap();
    file.seek(SeekFrom::End(-1)).unwrap();
    file.write_all(&[0xff]).unwrap();
    drop(file);
    assert!(
        encode_master(&mut master, StopToken::new(Unstoppable))
            .unwrap_err()
            .contains("refused")
    );
    assert!(reference.absolute_path.exists());
}

#[test]
fn cancellation_after_a_verified_master_read_stops_before_encoding_and_keeps_the_source() {
    struct CancelMaster<'a> {
        master: MasterBands<'a>,
        stop: Flag,
        reads: usize,
    }
    impl BandSource for CancelMaster<'_> {
        fn source(&self) -> PixelSource {
            self.master.source()
        }
        fn rows(&self) -> u32 {
            self.master.rows()
        }
        fn read_rows(&mut self, first: u32, rows: u32, out: &mut Vec<u8>) -> bool {
            self.reads += 1;
            let ok = self.master.read_rows(first, rows, out);
            self.stop.0.store(true, Ordering::Relaxed);
            ok
        }
    }
    let directory = Directory::new().unwrap();
    let reference = write_master(
        &CaptureArtifactStore::new(&directory.0),
        &mut SyntheticBands::new(133, 257),
    )
    .unwrap();
    let mut master = PixelMaster::open(&reference).unwrap();
    let stop = Flag(Arc::new(AtomicBool::new(false)));
    {
        let mut bands = CancelMaster {
            master: MasterBands(&mut master),
            stop: stop.clone(),
            reads: 0,
        };
        let error = encode(&mut bands, 133, StopToken::new(stop)).unwrap_err();
        assert!(error.to_lowercase().contains("cancel"));
        assert_eq!(bands.reads, 1);
    }
    assert_eq!(master.metadata().source, source());
    assert_eq!(master.read_rows(256, 1).unwrap().as_bytes().len(), 133 * 8);
    assert!(PixelMaster::open(&reference).is_ok());
}
