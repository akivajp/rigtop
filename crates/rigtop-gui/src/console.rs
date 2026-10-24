//! Console handling for a GUI binary.
//!
//! On Windows, release builds use the GUI subsystem: Explorer starts them
//! without a console window, but they also have no stdout/stderr. When
//! started from a terminal, the parent's console is attached so that
//! `--help`, `--version` and error messages still appear there; otherwise
//! errors are shown in a message box. Elsewhere this module just prints.

#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether a console is available for printing (Windows only).
#[cfg(windows)]
static HAS_CONSOLE: AtomicBool = AtomicBool::new(false);

/// Attaches to the console of the parent process (a terminal), if any.
#[cfg(windows)]
pub fn attach_parent() {
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetConsoleWindow,
    };
    // SAFETY: plain Win32 calls without pointers.
    let attached = unsafe {
        // Debug builds are console applications and already have one.
        !GetConsoleWindow().is_null() || AttachConsole(ATTACH_PARENT_PROCESS) != 0
    };
    HAS_CONSOLE.store(attached, Ordering::Relaxed);
}

/// Nothing to do outside Windows: stdout and stderr work as usual.
#[cfg(not(windows))]
pub fn attach_parent() {}

/// Reports a fatal error: on the console when there is one, otherwise (a
/// Windows GUI launched from Explorer) in a message box.
pub fn report_error(message: &str) {
    #[cfg(windows)]
    if !HAS_CONSOLE.load(Ordering::Relaxed) {
        show_message_box(message);
        return;
    }
    eprintln!("Error: {message}");
}

#[cfg(windows)]
fn show_message_box(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let text = wide(message);
    let caption = wide("rigtop");
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
