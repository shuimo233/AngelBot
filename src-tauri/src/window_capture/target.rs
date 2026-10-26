//! Scalar window identity only: no messages, UIA providers, titles or pixels.
//! UIA/password inspection and graphics capture remain in supervised children.

use super::{error, CaptureTarget};
use crate::desktop_control::DesktopAdapterError;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible, GW_OWNER,
};

pub(super) struct ReadOnlyProcess(HANDLE);

impl ReadOnlyProcess {
    pub(super) fn open(pid: u32) -> Result<Self, DesktopAdapterError> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return Err(error("TARGET_UNAVAILABLE"));
        }
        Ok(Self(handle))
    }

    pub(super) fn path(&self) -> Result<String, DesktopAdapterError> {
        let mut path = [0_u16; 4096];
        let mut length = path.len() as u32;
        let read = unsafe { QueryFullProcessImageNameW(self.0, 0, path.as_mut_ptr(), &mut length) };
        if read == 0 || length == 0 || length as usize > path.len() {
            return Err(error("TARGET_UNAVAILABLE"));
        }
        String::from_utf16(&path[..length as usize]).map_err(|_| error("TARGET_UNAVAILABLE"))
    }
}

impl Drop for ReadOnlyProcess {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

struct Selection<'a> {
    expected: &'a str,
    selected: Option<CaptureTarget>,
    failure: Option<DesktopAdapterError>,
    cancelled: &'a dyn Fn() -> bool,
    started: Instant,
    timeout: Duration,
}

impl Selection<'_> {
    fn check_budget(&self) -> Result<(), DesktopAdapterError> {
        if (self.cancelled)() {
            return Err(error("SCAN_CANCELLED"));
        }
        if self.started.elapsed() >= self.timeout {
            return Err(error("SCAN_TIMEOUT"));
        }
        Ok(())
    }

    fn consider(
        &mut self,
        visible: bool,
        ownerless: bool,
        load_candidate: impl FnOnce() -> Option<CaptureTarget>,
    ) -> Result<(), DesktopAdapterError> {
        if !visible || !ownerless {
            return Ok(());
        }
        let Some(candidate) = load_candidate() else {
            return Ok(());
        };
        if !candidate
            .executable_path
            .eq_ignore_ascii_case(self.expected)
        {
            return Ok(());
        }
        candidate.validate()?;
        if self.selected.is_some() {
            return Err(error("TARGET_AMBIGUOUS"));
        }
        self.selected = Some(candidate);
        Ok(())
    }
}

unsafe extern "system" fn consider_window(hwnd: HWND, context: LPARAM) -> i32 {
    let selection = unsafe { &mut *(context as *mut Selection<'_>) };
    let result = (|| {
        selection.check_budget()?;
        selection.consider(
            unsafe { IsWindowVisible(hwnd) } != 0,
            unsafe { GetWindow(hwnd, GW_OWNER) }.is_null(),
            || {
                let mut pid = 0;
                if unsafe { GetWindowThreadProcessId(hwnd, &mut pid) } == 0 || pid == 0 {
                    return None;
                }
                // Unreadable foreign processes were also excluded by the old selector.
                let executable_path = ReadOnlyProcess::open(pid)
                    .and_then(|process| process.path())
                    .ok()?;
                Some(CaptureTarget {
                    process_id: pid,
                    window_handle: hwnd as usize as u64,
                    executable_path,
                })
            },
        )
    })();
    match result {
        Ok(()) => 1,
        Err(failure) => {
            selection.failure = Some(failure);
            0
        }
    }
}

pub(crate) fn select_target(
    executable_path: &str,
    cancelled: &dyn Fn() -> bool,
    timeout: Duration,
) -> Result<CaptureTarget, DesktopAdapterError> {
    let mut selection = Selection {
        expected: executable_path,
        selected: None,
        failure: None,
        cancelled,
        started: Instant::now(),
        timeout,
    };
    selection.check_budget()?;
    let enumerated = unsafe {
        EnumWindows(
            Some(consider_window),
            (&mut selection as *mut Selection<'_>) as LPARAM,
        )
    };
    selection.check_budget()?;
    if let Some(failure) = selection.failure {
        return Err(failure);
    }
    if enumerated == 0 {
        return Err(error("TARGET_UNAVAILABLE"));
    }
    let target = selection
        .selected
        .ok_or_else(|| error("TARGET_NOT_FOUND"))?;
    if unsafe { IsIconic(target.window_handle as usize as HWND) } != 0 {
        return Err(error("TARGET_MINIMIZED"));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_requires_exact_path_and_one_hwnd_not_merely_one_process() {
        let expected = r"C:\fixture\trusted.exe";
        let mut selection = Selection {
            expected,
            selected: None,
            failure: None,
            cancelled: &|| false,
            started: Instant::now(),
            timeout: Duration::from_secs(1),
        };
        let candidate = |process_id, window_handle, executable_path: &str| CaptureTarget {
            process_id,
            window_handle,
            executable_path: executable_path.into(),
        };
        for (visible, ownerless) in [(false, true), (true, false), (false, false)] {
            selection
                .consider(visible, ownerless, || {
                    panic!("excluded windows must not read process identity")
                })
                .unwrap();
        }
        selection
            .consider(true, true, || {
                Some(candidate(99, 300, r"C:\fixture\other.exe"))
            })
            .unwrap();
        assert!(selection.selected.is_none());
        selection
            .consider(true, true, || {
                Some(candidate(42, 100, r"C:\FIXTURE\TRUSTED.EXE"))
            })
            .unwrap();
        assert_eq!(selection.selected.as_ref().unwrap().window_handle, 100);
        for pid in [42, 99] {
            assert_eq!(
                selection
                    .consider(true, true, || Some(candidate(pid, 200, expected)))
                    .unwrap_err()
                    .code,
                "TARGET_AMBIGUOUS"
            );
        }
        selection.selected = None;
        assert_eq!(
            selection
                .consider(true, true, || Some(candidate(0, 100, expected)))
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
        assert_eq!(
            selection
                .consider(true, true, || Some(candidate(42, 0, expected)))
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
        assert!(selection.selected.is_none());
        assert_eq!(
            select_target(expected, &|| true, Duration::from_secs(1))
                .err()
                .unwrap()
                .code,
            "SCAN_CANCELLED"
        );
        assert_eq!(
            select_target(expected, &|| false, Duration::ZERO)
                .err()
                .unwrap()
                .code,
            "SCAN_TIMEOUT"
        );
    }
}
