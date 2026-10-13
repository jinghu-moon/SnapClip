//! 贴图窗口的像素来源。
//!
//! 贴图窗口只消费一种东西：一块已解码的 32 位像素。它不关心像素是截图、
//! 剪贴板还是别的什么，所以这里把它定义成一个独立类型，而不是让窗口直接
//! 依赖截图模块。
//!
//! 为什么单独有这个文件：`host` 只负责「把这块像素变成屏幕上一个能拖动的
//! 窗口」，帧从哪来是调用方的事。这条边界让窗口实现不需要 `snapclip-capture`
//! 这个依赖，也让未来换来源（比如贴剪贴板里的图）不必动窗口代码。

use anyhow::{bail, Context, Result};

/// 一张已经解码好、可以直接往上画的图。
///
/// 像素布局是 Windows 的 32 位 DIB 布局：内存里按 BGRA 排列，每行之间没有
/// 额外补白，第 0 行是画面的最上面一行。这种格式可以直接喂给
/// `UpdateLayeredWindow`，贴图窗口内部不做任何颜色空间转换。
///
/// 用 `u32` 而不是 `u8` 持有像素，是因为要交给 `CreateDIBSection` 的缓冲区
/// 按 4 字节对齐。把对齐要求写进类型里，后面就不需要任何「这里其实是对齐的」
/// 式的断言。
#[derive(Debug)]
pub struct ImageSource {
    pixels: Vec<u32>,
    width: u32,
    height: u32,
}

impl ImageSource {
    /// 用一块 BGRA 像素建图，并**把 alpha 强制成不透明**。
    ///
    /// `pixels` 的长度必须恰好是 `width * height`，否则直接拒绝。与其把尺寸
    /// 不符的缓冲区带进窗口代码里（那会在别处以越界或错位的形式炸开），
    /// 不如在边界上就报错。
    ///
    /// 收的是 `Vec<u32>` 而不是字节，因为调用方拿到的本来就是 32 位像素
    /// （截图的选区缓冲）；每次过一遍字节只会白白多一次全图拷贝。
    pub fn from_bgra(pixels: Vec<u32>, width: u32, height: u32) -> Result<Self> {
        let mut image = Self::checked(pixels, width, height)?;
        image.make_opaque();
        Ok(image)
    }

    /// 从一段 BGRA **字节**建图，alpha 同样被强制成不透明。
    ///
    /// 这条路径存在的理由是贴图唯一的入口：捕获那边交出来的是 `&[u8]`（PNG 编码器和
    /// DIB 用的是同一块缓冲），而这里要在不复制整张图的前提下把它变成对齐的 `u32`。
    /// 逐 4 字节重新解释是这次转换唯一要做的事。
    pub fn from_bgra_bytes(bytes: &[u8], width: u32, height: u32) -> Result<Self> {
        if bytes.len() % 4 != 0 {
            bail!("BGRA 字节数必须是 4 的倍数，实际 {} 字节", bytes.len());
        }
        let pixels = bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        Self::from_bgra(pixels, width, height)
    }

    /// 从一段 RGBA 字节建图。
    ///
    /// 字节序是 R、G、B、A，alpha 保留——这是给**本来就带 alpha 的来源**
    /// （PNG 解码、剪贴板图像）用的通道，不是屏幕回读通道：屏幕回读走
    /// [`Self::from_bgra`]，那里 alpha 必须被强制成不透明。
    pub fn from_rgba(bytes: &[u8], width: u32, height: u32) -> Result<Self> {
        if bytes.len() % 4 != 0 {
            bail!("RGBA 字节数必须是 4 的倍数，实际 {} 字节", bytes.len());
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .context("图片尺寸溢出")?;
        if bytes.len() / 4 != expected {
            bail!(
                "像素个数与尺寸不符：{}x{} 需要 {expected} 个像素，实际 {} 个",
                width,
                height,
                bytes.len() / 4
            );
        }
        let pixels = bytes
            .chunks_exact(4)
            // RGBA 字节 → 内存里的 BGRA（小端 u32 的第 4 字节是 A）。
            .map(|chunk| u32::from_le_bytes([chunk[2], chunk[1], chunk[0], chunk[3]]))
            .collect();
        Self::checked(pixels, width, height)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// 像素数据，`height` 行，每行 `width` 个 BGRA 像素。
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// 把像素**搬走**，不复制，然后把自己留成一张空图。
    ///
    /// 收 `&mut self` 而不是 `self`：唯一的调用点在 `WM_NCCREATE` 里，那里手上只有
    /// `&mut WindowSeed`（`lpCreateParams` 递过来的裸指针换来的借用），拿不到
    /// `ImageSource` 的所有权。与其为了这个签名去 `Clone` 一整张 4K 图，不如在类型上
    /// 说清"搬走内容"这件事——`Vec` 的所有权转移一次也没多。
    ///
    /// 搬走之后 `self` 是一张 `0x0` 的空图：`pixels()` 返回空切片，`width()`/`height()`
    /// 都是 0。调用点必须在搬走**之前**把尺寸读出来，`attach_window` 就是这么做的。
    pub fn take_pixels(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.pixels)
    }

    fn checked(pixels: Vec<u32>, width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 {
            bail!("图片尺寸必须非零，收到 {width}x{height}");
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .context("图片尺寸溢出")?;
        if pixels.len() != expected {
            bail!(
                "像素个数与尺寸不符：{}x{} 需要 {expected} 个像素，实际 {} 个",
                width,
                height,
                pixels.len()
            );
        }
        Ok(Self {
            pixels,
            width,
            height,
        })
    }

    /// 把每个像素的 alpha 强制成 255。
    ///
    /// 这是**从捕获选区贴图的必经一步**，不是可选的美化。截图那条路径不写这个
    /// 字节：WGC 纹理回读到 CPU 时 alpha 常为 0，而分层窗口是按每个像素的 alpha
    /// 合成的，于是第 4 个字节为 0 的结果就是一块完全透明的窗口——看起来像
    /// 「贴图失败了」，实际是像素没被声明成不透明。
    fn make_opaque(&mut self) {
        for pixel in &mut self.pixels {
            *pixel |= 0xFF00_0000;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F6 那条路上两块最贵的搬运的净成本，和窗口、消息、GPU 都无关。
    ///
    /// 默认跳过（`#[ignore]`）：它是测量而不是断言，数字随机器变，进不了每次
    /// `cargo test`。要拿数字就这样跑：
    ///
    /// ```text
    /// cargo test --release -p snapclip-app --lib pin_bytes_to_u32_throughput -- --ignored --nocapture
    /// ```
    ///
    /// 两块搬运分别是：
    /// 1. `PinSource::take_selection` 把捕获侧的 `Vec<u8>` 复制一份交给贴图
    ///    （`Arc<Vec<u8>>` 装不下借来的切片，这次复制躲不掉）；
    /// 2. `ImageSource::from_bgra_bytes` 把它重解释成 `Vec<u32>`——**第二份
    ///    完整副本**，这是当前设计里最该被质疑的一处开销。
    #[test]
    #[ignore = "测量而不是断言；用 --ignored --nocapture 手动跑"]
    fn pin_bytes_to_u32_throughput() {
        use std::time::Instant;

        for (width, height) in [(1459u32, 1180u32), (3840, 2160)] {
            let bytes = vec![0xA5u8; width as usize * height as usize * 4];
            let rounds = 20;

            let started = Instant::now();
            let mut cloned = Vec::new();
            for _ in 0..rounds {
                cloned = bytes.clone();
            }
            let clone_us = started.elapsed().as_micros() / rounds as u128;

            let started = Instant::now();
            let mut image = None;
            for _ in 0..rounds {
                image = Some(ImageSource::from_bgra_bytes(&cloned, width, height).unwrap());
            }
            let convert_us = started.elapsed().as_micros() / rounds as u128;
            let image = image.unwrap();

            let mebibytes = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
            let mib = mebibytes(bytes.len());
            let clone_mib_per_s = mib / (clone_us as f64 / 1_000_000.0);
            let convert_mib_per_s = mib / (convert_us as f64 / 1_000_000.0);

            println!(
                "[pin-perf] size={width}x{height} bytes={:.1} MiB | \
                 pin_selection_clone={clone_us} us ({clone_mib_per_s:.0} MiB/s) | \
                 from_bgra_bytes={convert_us} us ({convert_mib_per_s:.0} MiB/s) | \
                 resident_pixels={:.1} MiB + u32_copy={:.1} MiB",
                mib,
                mebibytes(image.pixels().len() * 4),
                mebibytes(image.pixels().len() * 4),
            );
        }
    }

    #[test]
    fn accepts_matching_length() {
        let image = ImageSource::from_bgra(vec![0u32; 2 * 3], 2, 3).expect("尺寸匹配应当接受");
        assert_eq!(image.width(), 2);
        assert_eq!(image.height(), 3);
        assert_eq!(image.pixels().len(), 6);
    }

    #[test]
    fn rejects_zero_size() {
        assert!(ImageSource::from_bgra(Vec::new(), 0, 4).is_err());
        assert!(ImageSource::from_bgra(Vec::new(), 4, 0).is_err());
        assert!(ImageSource::from_rgba(&[], 0, 4).is_err());
    }

    #[test]
    fn rejects_length_mismatch() {
        let err = ImageSource::from_bgra(vec![0u32; 2], 3, 1)
            .expect_err("个数不符必须报错")
            .to_string();
        assert!(err.contains("像素个数与尺寸不符"), "实际错误：{err}");

        assert!(ImageSource::from_bgra(vec![0u32; 4], 3, 1).is_err());
    }

    #[test]
    fn bgra_input_alpha_is_forced_opaque() {
        // B=0x11, G=0x22, R=0x33, A=0x00 —— WGC 选区缓冲的典型样子。
        let image = ImageSource::from_bgra(vec![0x0033_2211], 1, 1).unwrap();
        assert_eq!(image.pixels()[0], 0xFF33_2211);
    }

    #[test]
    fn bgra_input_keeps_colour_bits() {
        let image = ImageSource::from_bgra(vec![0x4411_2233 & 0x00FF_FFFF], 1, 1).unwrap();
        assert_eq!(image.pixels()[0], 0xFF11_2233, "只有 alpha 应当被改写");
    }

    #[test]
    fn rgba_bytes_become_bgra_words() {
        // R=0x11, G=0x22, B=0x33, A=0x44 → 内存里 33 22 11 44。
        let image = ImageSource::from_rgba(&[0x11, 0x22, 0x33, 0x44], 1, 1).unwrap();
        assert_eq!(image.pixels()[0], 0x4411_2233);
    }

    #[test]
    fn rgba_bytes_reject_non_multiple_of_four() {
        assert!(ImageSource::from_rgba(&[0x11, 0x22, 0x33], 1, 1).is_err());
    }

    #[test]
    fn rgba_bytes_reject_length_mismatch() {
        assert!(ImageSource::from_rgba(&[0u8; 16], 3, 1).is_err());
    }
}
