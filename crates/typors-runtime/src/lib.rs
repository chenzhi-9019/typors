//! typors runtime DLL.
//!
//! The shipped `typors.exe` is a small loader; everything that pulls in GPUI and
//! the editor lives here. The DLL also owns the first-run shell registration.

mod app;
mod associate;

/// C entry point called by `typors-app`'s loader.
///
/// Returns the process exit code. Panics are caught here so a bug in the editor
/// cannot unwind across the DLL boundary.
#[unsafe(no_mangle)]
pub extern "C" fn typors_run() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let exe = std::env::current_exe().unwrap_or_default();

    if args.iter().any(|arg| arg == "--register") {
        return if associate::register_from_cli(&exe) { 0 } else { 1 };
    }
    if args.iter().any(|arg| arg == "--unregister") {
        return match associate::unregister() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("typors: cannot unregister Markdown handler: {error}");
                1
            }
        };
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        associate::maybe_register_on_first_run();
        app::run();
    }));
    match result {
        Ok(()) => 0,
        Err(payload) => {
            let message = if let Some(text) = payload.downcast_ref::<&str>() {
                (*text).to_string()
            } else if let Some(text) = payload.downcast_ref::<String>() {
                text.clone()
            } else {
                "unknown panic".to_string()
            };
            eprintln!("typors: fatal: {message}");
            1
        }
    }
}
