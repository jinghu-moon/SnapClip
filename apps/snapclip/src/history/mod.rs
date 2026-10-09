//! Clipboard history: the shell's first real capability (docs/23 T4.3).
//!
//! Organised by capability rather than by role, per the Coding Guides: the model, the
//! view and the icons live together, and nothing else in the app reaches into them.

pub mod card;
pub mod icons;
pub mod model;
pub mod preview;
pub mod rich;
pub mod view;
