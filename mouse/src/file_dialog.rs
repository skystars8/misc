use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
fn choose_macro_file(save: bool, suggested: Option<&Path>) -> Result<Option<PathBuf>, String> {
    use std::ffi::{OsStr, OsString};
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST,
        OFN_HIDEREADONLY, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    const BUFFER_LEN: usize = 32_768;

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    let mut file_buffer = vec![0_u16; BUFFER_LEN];
    let initial_name = suggested
        .map(Path::as_os_str)
        .or_else(|| save.then_some(OsStr::new("macro.mousemacro.txt")));
    if let Some(initial_name) = initial_name {
        let encoded = wide(initial_name);
        if encoded.len() > file_buffer.len() {
            return Err("the suggested macro path is too long for the file dialog".to_owned());
        }
        file_buffer[..encoded.len()].copy_from_slice(&encoded);
    }

    let filter: Vec<u16> = "Mouse macro text (*.mousemacro.txt;*.txt)\0*.mousemacro.txt;*.txt\0All files (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title = wide(OsStr::new(if save {
        "Save Mouse Macro"
    } else {
        "Load Mouse Macro"
    }));
    let default_extension = wide(OsStr::new("txt"));

    // OPENFILENAMEW is a plain Win32 C structure whose documented initial
    // state is all zeroes followed by the explicitly populated fields below.
    let mut dialog: OPENFILENAMEW = unsafe { zeroed() };
    dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    // The dialog is opened directly from an app button, so the foreground
    // window is the recorder and is a safe modal owner for this call.
    dialog.hwndOwner = unsafe { GetForegroundWindow() };
    dialog.lpstrFilter = filter.as_ptr();
    dialog.nFilterIndex = 1;
    dialog.lpstrFile = file_buffer.as_mut_ptr();
    dialog.nMaxFile = file_buffer.len() as u32;
    dialog.lpstrTitle = title.as_ptr();
    dialog.lpstrDefExt = default_extension.as_ptr();
    dialog.Flags = OFN_EXPLORER
        | OFN_HIDEREADONLY
        | OFN_NOCHANGEDIR
        | OFN_PATHMUSTEXIST
        | if save {
            OFN_OVERWRITEPROMPT
        } else {
            OFN_FILEMUSTEXIST
        };

    // The pointers above remain valid for the duration of this synchronous
    // call, and the API writes at most nMaxFile UTF-16 units to file_buffer.
    let accepted = unsafe {
        if save {
            GetSaveFileNameW(&mut dialog)
        } else {
            GetOpenFileNameW(&mut dialog)
        }
    } != 0;

    if !accepted {
        // A zero extended error is the documented representation of Cancel.
        let error_code = unsafe { CommDlgExtendedError() };
        return if error_code == 0 {
            Ok(None)
        } else {
            Err(format!(
                "Windows file dialog failed (error 0x{error_code:04X})"
            ))
        };
    }

    let path_len = file_buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(file_buffer.len());
    if path_len == 0 {
        return Err("the file dialog returned an empty path".to_owned());
    }

    Ok(Some(PathBuf::from(OsString::from_wide(
        &file_buffer[..path_len],
    ))))
}

#[cfg(not(target_os = "windows"))]
fn choose_macro_file(_save: bool, _suggested: Option<&Path>) -> Result<Option<PathBuf>, String> {
    Err("native macro file dialogs are only available on Windows".to_owned())
}

pub(crate) fn choose_save_path(suggested: Option<&Path>) -> Result<Option<PathBuf>, String> {
    choose_macro_file(true, suggested)
}

pub(crate) fn choose_load_path(suggested: Option<&Path>) -> Result<Option<PathBuf>, String> {
    choose_macro_file(false, suggested)
}
