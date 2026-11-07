//! Opening the configuration file, or its folder, in the system's apps:
//! on Windows it lives in `%APPDATA%`, which is hard to find by hand.

use std::path::Path;

/// Opens `path` in the default app for its type: on Windows the app
/// associated with `.toml`, else Notepad; on macOS the default text editor;
/// elsewhere what `xdg-open` picks.
pub fn open_file(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::open(path)
    }
    #[cfg(not(windows))]
    {
        let mut command = if cfg!(target_os = "macos") {
            let mut open = std::process::Command::new("open");
            // -t: the default text editor, whatever owns .toml.
            open.arg("-t");
            open
        } else {
            std::process::Command::new("xdg-open")
        };
        command
            .arg(path)
            .spawn()
            .map(drop)
            .map_err(|err| format!("cannot open {}: {err}", path.display()))
    }
}

/// Opens a folder in the system file manager.
pub fn open_folder(folder: &Path) -> Result<(), String> {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(folder)
        .spawn()
        .map(drop)
        .map_err(|err| format!("cannot open {}: {err}", folder.display()))
}

#[cfg(windows)]
mod windows {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    /// ShellExecute's answer when no app is associated with the file type.
    const SE_ERR_NOASSOC: isize = 31;

    fn wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(Some(0)).collect()
    }

    /// Whether an app opens files ending in `extension` (".toml"). Without
    /// `ASSOCF_INIT_IGNOREUNKNOWN`, Windows answers with its "choose an app"
    /// dialog (OpenWith.exe) when nothing is associated.
    fn associated(extension: &OsStr) -> bool {
        use windows_sys::Win32::UI::Shell::{
            ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_EXECUTABLE, AssocQueryStringW,
        };
        let extension = wide(extension);
        let verb = wide(OsStr::new("open"));
        let mut size = 0u32;
        // SAFETY: the strings are NUL-terminated and outlive the call; no
        // output buffer is passed, only asked for the size it would need.
        let result = unsafe {
            AssocQueryStringW(
                ASSOCF_INIT_IGNOREUNKNOWN,
                ASSOCSTR_EXECUTABLE,
                extension.as_ptr(),
                verb.as_ptr(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        result >= 0
    }

    fn notepad(path: &Path) -> Result<(), String> {
        std::process::Command::new("notepad.exe")
            .arg(path)
            .spawn()
            .map(drop)
            .map_err(|err| format!("cannot open {} in Notepad: {err}", path.display()))
    }

    pub fn open(path: &Path) -> Result<(), String> {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        // Nothing opens .toml files on a fresh Windows, and ShellExecute
        // would then show the "choose an app" dialog and report success.
        let extension = path.extension().map(|extension| {
            let mut dotted = OsString::from(".");
            dotted.push(extension);
            dotted
        });
        if !extension.as_deref().is_some_and(associated) {
            return notepad(path);
        }
        let verb = wide(OsStr::new("open"));
        let file = wide(path.as_os_str());
        // SAFETY: the strings are NUL-terminated and outlive the call.
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        } as isize;
        match result {
            // Values above 32 mean success.
            r if r > 32 => Ok(()),
            SE_ERR_NOASSOC => notepad(path),
            r => Err(format!("cannot open {} (error {r})", path.display())),
        }
    }
}
