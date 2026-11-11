//! Native Open/Save dialogs. `windows-sys` keeps the installer payload free of
//! another UI crate; the dialogs are modal and run on the editor thread.

use std::path::PathBuf;

use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
    OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};

const FILTER: &str = "Markdown\0*.md;*.markdown;*.mdown;*.txt\0All files\0*.*\0\0";

pub fn open(title: &str) -> Option<PathBuf> {
    dialog(title, false)
}

pub fn save(title: &str) -> Option<PathBuf> {
    dialog(title, true)
}

fn dialog(title: &str, save: bool) -> Option<PathBuf> {
    let filter = wide(FILTER);
    let title = wide(title);
    let default_ext = wide("md");
    let mut file = vec![0u16; 32 * 1024];
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: file.len() as u32,
        lpstrTitle: title.as_ptr(),
        lpstrDefExt: default_ext.as_ptr(),
        Flags: OFN_EXPLORER | OFN_NOCHANGEDIR | OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST,
        ..Default::default()
    };
    if save {
        dialog.Flags |= OFN_OVERWRITEPROMPT;
    }
    let accepted = unsafe {
        if save {
            GetSaveFileNameW(&mut dialog)
        } else {
            GetOpenFileNameW(&mut dialog)
        }
    };
    if accepted == 0 {
        return None;
    }
    let length = file.iter().position(|unit| *unit == 0).unwrap_or(0);
    if length == 0 {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(&file[..length])))
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::wide;

    #[test]
    fn filter_is_double_nul_terminated() {
        let parsed: Vec<&str> = super::FILTER.split('\0').collect();
        assert!(parsed.len() >= 5);
        assert_eq!(parsed.last(), Some(&""));
    }

    #[test]
    fn wide_strings_are_nul_terminated() {
        let units = wide("typors");
        assert_eq!(units.last(), Some(&0));
        assert_eq!(units.len(), 7);
    }
}
