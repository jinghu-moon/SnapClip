//! Infrastructure layer: adapters that talk to the outside world (disk, database,
//! image codecs). Depends on `domain` only.

pub mod image;
pub mod store;
