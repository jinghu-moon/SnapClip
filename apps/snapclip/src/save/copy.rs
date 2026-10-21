//! Re-export of the shared validation copy.
//!
//! The strings themselves live in snapclip_ui::copy, because the font subset's codepoint list is
//! built from that module (overlay_drawn_strings() in snapclip-capture calls its producers).
//! Keeping this thin re-export means the save service and the panel keep writing super::copy,
//! while there is still exactly one place that spells the sentences.

pub use snapclip_ui::copy::*;