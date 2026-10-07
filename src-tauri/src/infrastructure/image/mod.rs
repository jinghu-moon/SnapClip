//! 过渡期转发（docs/23 T2.4）：编解码实现已搬到 `snapclip-history`，这里只做名字转发，
//! 让壳侧仍在写的 `crate::infrastructure::image::…` 路径可以编译。
//! 删除条件：T2.5/T2.6 把壳里最后两个使用者（artifact 编码器、clipboard 适配器）迁走之后。

pub use snapclip_history::image::{
    Bgra8Image, MAX_DECODE_PIXELS, decode_to_bgra8, decode_to_rgba8, downscale_rgba_to_max_side,
    encode_png, encode_rgba_png, png_dimensions,
};
