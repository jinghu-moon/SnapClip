mod encode;

pub use encode::{
    Bgra8Image, MAX_DECODE_PIXELS, decode_to_bgra8, decode_to_rgba8, downscale_rgba_to_max_side,
    encode_png, encode_rgba_png, png_dimensions,
};