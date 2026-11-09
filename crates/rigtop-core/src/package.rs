//! Running from an MSIX package (the Microsoft Store), and what changes
//! then.
//!
//! The same executables run from the zip, Scoop and winget too. Packaged,
//! Windows redirects the app's writes to `%APPDATA%` and `%LOCALAPPDATA%`
//! into a private copy that only the package sees: the configuration file
//! would then be invisible to Notepad or an editor opened from the GUI. So,
//! packaged, rigtop keeps its files in the package's own folder
//! (`LocalState`, a real folder every program can open), and the Store, not
//! rigtop, brings new versions.
//!
//! Elsewhere (Linux, macOS, Windows outside a package) everything here says
//! "not packaged".

use std::path::PathBuf;
use std::sync::OnceLock;

/// The package family name (`C2B504BA.rigtop_1raghr5p0ryyw`), if running
/// from a package. Looked up once: it cannot change while running.
pub fn family_name() -> Option<&'static str> {
    static NAME: OnceLock<Option<String>> = OnceLock::new();
    NAME.get_or_init(query_family_name).as_deref()
}

/// Whether running from an MSIX package (the Microsoft Store's copy).
pub fn is_packaged() -> bool {
    family_name().is_some()
}

/// The package's folder for its own files,
/// `%LOCALAPPDATA%\Packages\<family name>\LocalState`, if packaged.
pub fn local_state() -> Option<PathBuf> {
    let family = family_name()?;
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(local)
            .join("Packages")
            .join(family)
            .join("LocalState"),
    )
}

/// Asks Windows for the family name of the package this process runs in.
#[cfg(windows)]
fn query_family_name() -> Option<String> {
    use windows_sys::Win32::Foundation::{
        APPMODEL_ERROR_NO_PACKAGE, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS,
    };
    use windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFamilyName;

    let mut length = 0u32;
    // SAFETY: a size query (no buffer); Windows writes the length needed.
    let result = unsafe { GetCurrentPackageFamilyName(&mut length, std::ptr::null_mut()) };
    // Outside a package (the usual case), or an error: not packaged.
    if result == APPMODEL_ERROR_NO_PACKAGE || result != ERROR_INSUFFICIENT_BUFFER || length == 0 {
        return None;
    }
    let mut buffer = vec![0u16; length as usize];
    // SAFETY: the buffer holds `length` UTF-16 units, as Windows asked for.
    let result = unsafe { GetCurrentPackageFamilyName(&mut length, buffer.as_mut_ptr()) };
    if result != ERROR_SUCCESS {
        return None;
    }
    // `length` counts the terminating zero.
    Some(String::from_utf16_lossy(
        &buffer[..(length as usize).saturating_sub(1)],
    ))
}

/// Only Windows has packages.
#[cfg(not(windows))]
fn query_family_name() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_do_not_run_from_a_package() {
        // `cargo test` runs a plain executable on every platform.
        assert!(!is_packaged());
        assert_eq!(local_state(), None);
    }
}
