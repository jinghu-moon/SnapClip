//! Binary entry point. Everything it does lives in the library so the shell can be
//! tested (docs/23 T4.3, the GPUI test layer).

fn main() {
    snapclip_app::run();
}
