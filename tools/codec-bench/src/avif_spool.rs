//! The product codec's disk-backed export path, measured without a whole image.
use almost_enough::StopToken;
use enough::Unstoppable;
use rgb::Rgba;
use std::{
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

pub(super) fn resources(width: u32, height: u32, path: &Path) -> Result<(), String> {
    let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut fill_time = Duration::ZERO;
    let mut tiles = 0usize;
    let mut random = 0x941c_f17a_u32;
    let start = Instant::now();
    let (result, peak) = super::alloc_probe::peak_extra(|| {
        snapclip_avif::write_pq_tiles(
            width,
            height,
            path.parent().ok_or("missing scratch directory")?,
            &mut file,
            StopToken::new(Unstoppable),
            |_, _, tile| {
                let start = Instant::now();
                for _ in 0..128 * 128 {
                    let mut sample = || {
                        random ^= random << 13;
                        random ^= random >> 17;
                        random ^= random << 5;
                        let v = (random & 1023) as u16;
                        (v << 6) | (v >> 4)
                    };
                    tile.push(Rgba::new(sample(), sample(), sample(), sample()));
                }
                tiles += 1;
                fill_time += start.elapsed();
                Ok(())
            },
        )
        .map_err(|e| e.to_string())
    });
    let largest = super::alloc_probe::largest_allocation();
    let elapsed = start.elapsed();
    let bytes = result?;
    file.flush().map_err(|e| e.to_string())?;
    if peak > 32 * 1024 * 1024 || largest >= 16 * 1024 * 1024 {
        return Err(format!("spool memory budget exceeded {peak}/{largest}"));
    }
    if bytes <= 4 * 1024 * 1024 || file.metadata().map_err(|e| e.to_string())?.len() != bytes {
        return Err("spool did not demonstrate complete output beyond former 4 MiB cap".into());
    }
    println!(
        "{{\"width\":{width},\"height\":{height},\"tiles\":{tiles},\"bytes\":{bytes},\"extra_heap\":{peak},\"largest_allocation\":{largest},\"elapsed_ms\":{:.3},\"fill_ms\":{:.3},\"codec_spool_ms\":{:.3}}}",
        elapsed.as_secs_f64() * 1000.,
        fill_time.as_secs_f64() * 1000.,
        elapsed.saturating_sub(fill_time).as_secs_f64() * 1000.
    );
    Ok(())
}

pub(super) fn decode(input: &Path, output: &Path) -> Result<(), String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(input)
        .map_err(|e| e.to_string())?
        .take(snapclip_avif::MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    let (pixels, peak) =
        super::alloc_probe::peak_extra(|| snapclip_avif::decode(&bytes, &Unstoppable));
    let pixels = pixels.map_err(|e| {
        println!(
            "{{\"rejected\":true,\"extra_heap\":{peak},\"largest_allocation\":{}}}",
            super::alloc_probe::largest_allocation()
        );
        e.to_string()
    })?;
    std::fs::write(output, pixels.as_bytes()).map_err(|e| e.to_string())?;
    println!(
        "{{\"width\":{},\"height\":{},\"hdr\":{},\"extra_heap\":{peak},\"elapsed_ms\":{:.3}}}",
        pixels.size()[0],
        pixels.size()[1],
        pixels.descriptor().encoding() == snapclip_model::pixels::PixelEncoding::ScRgb,
        start.elapsed().as_secs_f64() * 1000.
    );
    Ok(())
}
