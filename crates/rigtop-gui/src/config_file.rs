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
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    /// ShellExecute's answer when no app is associated with the file type.
    const SE_ERR_NOASSOC: isize = 31;

    pub fn open(path: &Path) -> Result<(), String> {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let wide = |s: &OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
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
            // Nothing opens .toml files on a fresh Windows: use Notepad.
            SE_ERR_NOASSOC => std::process::Command::new("notepad.exe")
                .arg(path)
                .spawn()
                .map(drop)
                .map_err(|err| format!("cannot open {} in Notepad: {err}", path.display())),
            r => Err(format!("cannot open {} (error {r})", path.display())),
        }
    }
}
