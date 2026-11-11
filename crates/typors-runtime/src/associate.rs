//! Windows shell registration for Markdown.
//!
//! Windows 10/11 deliberately do not let an application silently become the
//! default handler (the `UserChoice` hash is user-owned). What this module can do
//! is register typors as a *candidate* handler for `.md` and open the Default
//! Apps page so the user can confirm it. The first-run prompt asks first.

use std::path::Path;
use std::process::Command;

use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO, MessageBoxW};
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

const PROG_ID: &str = "typors.md";
const APP_NAME: &str = "typors";
const FIRST_RUN_VALUE: &str = "AssociationPrompted";

/// Registration scope chosen by the installer: current user or every user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    User,
    Machine,
}

fn root(scope: Scope) -> RegKey {
    match scope {
        Scope::User => RegKey::predef(HKEY_CURRENT_USER),
        Scope::Machine => RegKey::predef(HKEY_LOCAL_MACHINE),
    }
}

/// Register typors as a Markdown candidate handler and record that the first-run
/// prompt has been dealt with.
pub fn register(exe: &Path, scope: Scope) -> std::io::Result<()> {
    let root = root(scope);
    let classes = root.create_subkey("Software\\Classes")?.0;

    let prog = classes.create_subkey(PROG_ID)?.0;
    prog.set_value("", &"Markdown Document")?;
    prog.create_subkey("DefaultIcon")?
        .0
        .set_value("", &format!("\"{}\",0", exe.display()))?;
    prog.create_subkey("shell\\open\\command")?
        .0
        .set_value("", &format!("\"{}\" \"%1\"", exe.display()))?;

    // Advertise the ProgID without stealing an existing user's `.md` choice.
    let md = classes.create_subkey(".md")?.0;
    let current = md.get_value::<String, _>("").unwrap_or_default();
    if current.is_empty() || current == PROG_ID {
        md.set_value("", &PROG_ID)?;
    }
    md.create_subkey("OpenWithProgids")?
        .0
        .set_value(PROG_ID, &"")?;

    classes
        .create_subkey("Applications\\typors.exe\\shell\\open\\command")?
        .0
        .set_value("", &format!("\"{}\" \"%1\"", exe.display()))?;

    let capabilities = root.create_subkey("Software\\typors\\Capabilities")?.0;
    capabilities.set_value("ApplicationName", &"typors")?;
    capabilities.set_value("ApplicationDescription", &"typors Markdown editor")?;
    capabilities
        .create_subkey("FileAssociations")?
        .0
        .set_value(".md", &PROG_ID)?;

    root.create_subkey("Software\\RegisteredApplications")?
        .0
        .set_value(APP_NAME, &"Software\\typors\\Capabilities")?;

    if scope == Scope::User {
        mark_first_run_done()?;
    }
    Ok(())
}

/// `--register` / `--register-all` from the installer: register without prompting.
pub fn register_from_cli(exe: &Path, scope: Scope) -> bool {
    match register(exe, scope) {
        Ok(()) => {
            println!("typors: registered as a Markdown (.md) candidate handler ({scope:?})");
            true
        }
        Err(error) => {
            eprintln!("typors: cannot register Markdown handler: {error}");
            false
        }
    }
}

/// On the first normal launch, ask before touching the shell, then point the user
/// at Default Apps (the only place Windows lets the default be confirmed).
pub fn maybe_register_on_first_run() {
    if std::env::args().any(|arg| {
        arg == "--register"
            || arg == "--register-all"
            || arg == "--unregister"
            || arg == "--unregister-all"
            || arg == "--no-associate"
            || arg.starts_with("--dump")
            || arg == "--fps"
    }) {
        return;
    }
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if hkcu
        .open_subkey("Software\\typors")
        .and_then(|key| key.get_value::<u32, _>(FIRST_RUN_VALUE))
        .is_ok_and(|value| value == 1)
    {
        return;
    }

    let answer = unsafe { ask_user() };
    if answer == IDYES {
        if let Ok(exe) = std::env::current_exe()
            && register(&exe, Scope::User).is_ok()
        {
            open_default_apps();
        }
    }
    let _ = mark_first_run_done();
}

fn mark_first_run_done() -> std::io::Result<()> {
    RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey("Software\\typors")?
        .0
        .set_value(FIRST_RUN_VALUE, &1u32)
}

/// Build the native Yes/No prompt. This is the one unavoidable `unsafe` call in
/// the runtime: `MessageBoxW` is a C API with two NUL-terminated strings.
#[allow(unsafe_code)]
unsafe fn ask_user() -> i32 {
    let text: Vec<u16> = "将 typors 注册为 Markdown (.md) 文件的打开方式？\n\n\
        Windows 可能还会要求你在“默认应用”中确认。"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let title: Vec<u16> = "typors".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_ICONQUESTION | MB_YESNO,
        )
    }
}

fn open_default_apps() {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = Command::new("cmd")
        .args(["/C", "start", "ms-settings:defaultapps"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// Remove the candidate registration. Used by the NSIS uninstaller via
/// `typors.exe --unregister` / `--unregister-all`.
pub fn unregister(scope: Scope) -> std::io::Result<()> {
    let root = root(scope);
    let classes = root.open_subkey("Software\\Classes")?;
    let _ = classes.delete_subkey_all(PROG_ID);
    let _ = classes.delete_subkey_all("Applications\\typors.exe");
    if let Ok(md) = classes.open_subkey(".md") {
        let current = md.get_value::<String, _>("").unwrap_or_default();
        if current == PROG_ID {
            let _ = md.delete_value("");
        }
        if let Ok(open_with) = md.open_subkey("OpenWithProgids") {
            let _ = open_with.delete_value(PROG_ID);
        }
    }
    if let Ok(registered) = root.open_subkey("Software\\RegisteredApplications") {
        let _ = registered.delete_value(APP_NAME);
    }
    let _ = root.delete_subkey_all("Software\\typors");
    Ok(())
}
