//! Silent Windows desktop entry point for Anonymix Scoobs V1.8 Rust port.
//! No console, stdout/stderr reports, or default Rust panic-hook text is emitted.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    // catch_unwind in the C callbacks still enforces the existing shutdown
    // boundary. Suppress only the default text hook, not error containment.
    std::panic::set_hook(Box::new(|_| {}));
    #[cfg(all(windows, target_pointer_width = "64"))]
    if anonymix_v18_dsp_port::desktop::run().is_err() {
        std::process::exit(1);
    }
    #[cfg(not(all(windows, target_pointer_width = "64")))]
    std::process::exit(1);
}
