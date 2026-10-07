//! Content-addressed clipboard payload storage (docs/23 T2.3).
//!
//! Moved from `src-tauri/src/infrastructure/store/blob.rs`, semantics unchanged: the
//! `blake3` of the bytes is the address (`{hash[..2]}/{hash}.blob`), identical bytes are
//! stored once, and every read re-verifies the hash before handing the bytes back.
