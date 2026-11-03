//! CPU AVIF capability. Source pixels stay immutable; the caller owns atomic
//! publication and background execution. No GPUI or display dependency.
#![cfg(windows)]

mod bands;
mod decode;
mod encode;
pub use almost_enough::{StopToken, Stopper};
pub use bands::write_hdr;
pub use decode::{MAX_INPUT_BYTES, MAX_PREVIEW_PIXELS, decode, is_avif};
pub use encode::{MAX_SOURCE_READ_BYTES, TILE_SIDE, within_export_geometry, write_pq_tiles};
pub use enough::{Stop, Unstoppable};

#[derive(Debug)]
pub enum Error {
    Cancelled,
    Invalid(String),
    Io(std::io::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Invalid(e) => f.write_str(e),
            Self::Io(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<&str> for Error {
    fn from(e: &str) -> Self {
        Self::Invalid(e.into())
    }
}
impl From<String> for Error {
    fn from(e: String) -> Self {
        Self::Invalid(e)
    }
}
impl Error {
    pub fn into_io(self) -> std::io::Error {
        match self {
            Self::Io(e) => e,
            Self::Cancelled => std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"),
            Self::Invalid(e) => std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        }
    }
}
pub type Result<T> = std::result::Result<T, Error>;
fn check(stop: &(impl Stop + ?Sized)) -> Result<()> {
    stop.check().map_err(|_| Error::Cancelled)
}

#[cfg(test)]
mod tests;
