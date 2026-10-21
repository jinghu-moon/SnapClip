//! Binary entry point. Everything it does lives in the library so the shell can be
//! tested (docs/23 T4.3, the GPUI test layer).

fn main() {
    // Development entry (`docs/41` S2): with `SNAPCLIP_OPEN_SAVE_PANEL` set, open the save panel
    // directly instead of running the application, so the panel can be looked at while it is being
    // built. It is not a product path and S10 replaces it by opening the panel from the capture
    // flow. The panel comes up with no capture behind it: the preview column is empty.
    if std::env::var_os("SNAPCLIP_OPEN_SAVE_PANEL").is_some() {
        match snapclip_app::save::window::open(Default::default()) {
            Ok(shell) => {
                eprintln!("[save-panel] development entry: close the window to exit");
                shell.wait();
            }
            Err(error) => eprintln!("[save-panel] development entry failed: {error:?}"),
        }
        return;
    }
    snapclip_app::run();
}
