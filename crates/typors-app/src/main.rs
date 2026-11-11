//! The shipped executable is a loader: `typors_runtime.dll` next to it owns the
//! editor, GPUI and the shell registration. Keeping the runtime in a DLL is what
//! keeps this EXE small and lets the installer ship icons/assets as files.
//!
//! Windows GUI subsystem: double-clicking typors must never open a black console
//! window. Stdout/stderr handles still work when the EXE is started from a
//! terminal or redirected, so `--dump-*` keeps its output.

#![windows_subsystem = "windows"]

use std::env;
use std::path::PathBuf;

fn main() {
    let exe = env::current_exe().unwrap_or_else(|error| {
        eprintln!("typors: cannot locate executable: {error}");
        std::process::exit(2);
    });
    let dll = dll_path(&exe);
    let library = unsafe { libloading::Library::new(&dll) }.unwrap_or_else(|error| {
        eprintln!("typors: cannot load runtime DLL {}: {error}", dll.display());
        std::process::exit(3);
    });
    let run: libloading::Symbol<extern "C" fn() -> i32> = unsafe { library.get(b"typors_run\0") }
        .unwrap_or_else(|error| {
            eprintln!("typors: runtime DLL has no typors_run entry: {error}");
            std::process::exit(4);
        });
    let code = run();
    std::process::exit(code);
}

fn dll_path(exe: &PathBuf) -> PathBuf {
    let dir = exe.parent().unwrap_or_else(|| std::path::Path::new("."));
    dir.join("typors_runtime.dll")
}
