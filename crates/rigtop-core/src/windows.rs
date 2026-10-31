//! Windows-specific pieces: ending processes and network link speeds.
//!
//! Windows has no signals. A forced kill maps to `TerminateProcess`; a polite
//! "terminate" is approximated by asking the process's top-level windows to
//! close (`WM_CLOSE`), which is what Task Manager's "End task" does for
//! applications. Console programs and services have no window to close.

use std::collections::HashMap;
use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows_sys::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
};
use windows_sys::core::BOOL;

use crate::snapshot::NetworkStats;
use crate::source::ProcessSignal;

/// Ends a process: `Kill` terminates it immediately, `Terminate` asks its
/// windows to close and fails if it has none.
pub(crate) fn send_signal(pid: u32, signal: ProcessSignal) -> io::Result<()> {
    // pid 0 is the System Idle Process and 4 the kernel; never target them.
    if pid <= 4 {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    match signal {
        ProcessSignal::Kill => terminate_process(pid),
        ProcessSignal::Terminate => close_windows(pid),
        // Pausing and priorities are Unix only for now.
        ProcessSignal::Stop | ProcessSignal::Continue | ProcessSignal::Nice(_) => {
            Err(io::Error::from(io::ErrorKind::Unsupported))
        }
    }
}

fn terminate_process(pid: u32) -> io::Result<()> {
    // SAFETY: plain Win32 calls; the handle is closed on every path.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let ok = TerminateProcess(handle, 1) != 0;
        let error = io::Error::last_os_error();
        CloseHandle(handle);
        if ok { Ok(()) } else { Err(error) }
    }
}

/// State shared with the `EnumWindows` callback.
struct CloseRequest {
    pid: u32,
    posted: usize,
}

unsafe extern "system" fn post_close(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `CloseRequest` passed by `close_windows`,
    // alive for the whole enumeration.
    let request = unsafe { &mut *(lparam as *mut CloseRequest) };
    let mut owner = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
    if owner == request.pid && unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) } != 0 {
        request.posted += 1;
    }
    1 // continue enumerating
}

fn close_windows(pid: u32) -> io::Result<()> {
    let mut request = CloseRequest { pid, posted: 0 };
    // SAFETY: the callback only touches `request` during this call.
    unsafe {
        EnumWindows(
            Some(post_close),
            &mut request as *mut CloseRequest as LPARAM,
        )
    };
    if request.posted > 0 {
        Ok(())
    } else {
        Err(io::Error::other(
            "the process has no window to close; use force kill instead",
        ))
    }
}

/// Fills `link_speed_mbps` and `link_up` from `GetIfTable2`, matching
/// interfaces by alias (the name `sysinfo` reports on Windows).
pub(crate) fn annotate_links(networks: &mut [NetworkStats]) {
    let links = read_links();
    for net in networks {
        if let Some(&(speed, up)) = links.get(&net.name) {
            net.link_speed_mbps = speed;
            net.link_up = Some(up);
        }
    }
}

/// Alias -> (receive link speed in Mbit/s, operationally up).
fn read_links() -> HashMap<String, (Option<u32>, bool)> {
    let mut links = HashMap::new();
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: GetIfTable2 allocates the table, freed with FreeMibTable below;
    // rows are read within NumEntries.
    unsafe {
        if GetIfTable2(&mut table) != 0 || table.is_null() {
            return links;
        }
        let count = (*table).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), count);
        for row in rows {
            let len = row
                .Alias
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(row.Alias.len());
            let alias = String::from_utf16_lossy(&row.Alias[..len]);
            let up = row.OperStatus == IfOperStatusUp;
            // Speeds are in bit/s; disconnected adapters report 0 or u64::MAX.
            let speed = (row.ReceiveLinkSpeed > 0 && row.ReceiveLinkSpeed != u64::MAX)
                .then(|| u32::try_from(row.ReceiveLinkSpeed / 1_000_000).ok())
                .flatten();
            links.insert(alias, (speed, up));
        }
        FreeMibTable(table as *const _);
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn spawn_console_child() -> std::process::Child {
        // A console program with no window: `ping` waits about 30 s.
        Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn ping")
    }

    #[test]
    fn refuses_system_pids() {
        for pid in [0, 4] {
            let err = send_signal(pid, ProcessSignal::Kill).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn terminate_needs_a_window_and_kill_ends_the_process() {
        let mut child = spawn_console_child();
        // No window to close: polite termination is reported as impossible.
        let err = send_signal(child.id(), ProcessSignal::Terminate).unwrap_err();
        assert!(err.to_string().contains("no window"), "{err}");
        send_signal(child.id(), ProcessSignal::Kill).unwrap();
        assert!(!child.wait().unwrap().success());
    }

    #[test]
    fn link_table_is_readable() {
        // Every Windows machine has at least the loopback interface.
        assert!(!read_links().is_empty());
    }
}
