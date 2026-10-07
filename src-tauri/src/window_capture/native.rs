//! Windows-only WGC and scalar safety checks. All blocking calls run inside the
//! supervised same-executable helper, never on the parent agent's runtime thread.

use super::{
    copy_rgba_rows, encode_rgba, error, validate_frame_size, validate_source_dimensions,
    validate_visible_pixels, CaptureTarget,
};
use crate::desktop_control::{capture_target_probe, DesktopAdapterError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl};
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    GraphicsCaptureItemType, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::RemoteDesktop::{
    ProcessIdToSessionId, WTSActive, WTSFreeMemory, WTSQuerySessionInformationW, WTSSessionInfoEx,
    WTSINFOEXW, WTS_SESSIONSTATE_UNLOCK,
};
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_READOBJECTS, UOI_NAME,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowDisplayAffinity, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    WDA_NONE,
};

// windows-sys does not expose WinRT initialization. These are only the two
// official runtime lifecycle functions; WGC/D3D remains in windows-capture.
#[link(name = "runtimeobject")]
unsafe extern "system" {
    fn RoInitialize(init_type: u32) -> i32;
    fn RoUninitialize();
}

struct WinRt;
impl WinRt {
    fn new() -> Result<Self, DesktopAdapterError> {
        // RO_INIT_MULTITHREADED = 1. S_OK and S_FALSE both require uninitializing.
        if unsafe { RoInitialize(1) } < 0 {
            return Err(error("UNSUPPORTED_PLATFORM"));
        }
        Ok(Self)
    }
}
impl Drop for WinRt {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn scalar_guard(target: &CaptureTarget) -> Result<(), DesktopAdapterError> {
    let hwnd = target.window_handle as usize as *mut std::ffi::c_void;
    let mut pid = 0;
    unsafe {
        if IsWindow(hwnd) == 0
            || IsWindowVisible(hwnd) == 0
            || IsIconic(hwnd) != 0
            || GetWindowThreadProcessId(hwnd, &mut pid) == 0
            || pid != target.process_id
        {
            return Err(error("TARGET_CHANGED"));
        }
    }
    let process = Handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) });
    if process.0.is_null() {
        return Err(error("TARGET_UNAVAILABLE"));
    }
    let mut path = [0_u16; 4096];
    let mut length = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut length) } == 0
        || length == 0
        || length as usize > path.len()
    {
        return Err(error("TARGET_UNAVAILABLE"));
    }
    let path =
        String::from_utf16(&path[..length as usize]).map_err(|_| error("TARGET_UNAVAILABLE"))?;
    if !path.eq_ignore_ascii_case(&target.executable_path) {
        return Err(error("TARGET_CHANGED"));
    }
    session_guard(pid)?;
    let mut affinity = 0;
    let known = unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) } != 0;
    if known && affinity != WDA_NONE {
        return Err(error("SENSITIVE_SURFACE"));
    }
    // Query failure is Unknown, not Unprotected. Only OS WGC captures this
    // window, respecting its protection; no PrintWindow/BitBlt/monitor fallback.
    Ok(())
}

fn session_guard(target_pid: u32) -> Result<(), DesktopAdapterError> {
    let mut own_session = 0;
    let mut target_session = 0;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut own_session) } == 0
        || unsafe { ProcessIdToSessionId(target_pid, &mut target_session) } == 0
        || own_session != target_session
    {
        return Err(error("TARGET_UNAVAILABLE"));
    }
    let mut buffer = std::ptr::null_mut();
    let mut length = 0;
    let succeeded = unsafe {
        WTSQuerySessionInformationW(
            std::ptr::null_mut(),
            own_session,
            WTSSessionInfoEx,
            &mut buffer,
            &mut length,
        )
    } != 0;
    struct WtsBuffer(*mut u16);
    impl Drop for WtsBuffer {
        fn drop(&mut self) {
            unsafe {
                WTSFreeMemory(self.0.cast());
            }
        }
    }
    let buffer = WtsBuffer(buffer);
    if !succeeded || buffer.0.is_null() || (length as usize) < std::mem::size_of::<WTSINFOEXW>() {
        return Err(error("TARGET_UNAVAILABLE"));
    }
    let info = unsafe { std::ptr::read_unaligned(buffer.0.cast::<WTSINFOEXW>()) };
    if !valid_session_info(&info, own_session) {
        return Err(error("TARGET_UNAVAILABLE"));
    }
    let desktop = unsafe { OpenInputDesktop(0, 0, DESKTOP_READOBJECTS) };
    if desktop.is_null() {
        return Err(error("TARGET_UNAVAILABLE"));
    }
    struct InputDesktop(windows_sys::Win32::System::StationsAndDesktops::HDESK);
    impl Drop for InputDesktop {
        fn drop(&mut self) {
            unsafe {
                CloseDesktop(self.0);
            }
        }
    }
    let desktop = InputDesktop(desktop);
    let mut name = [0_u16; 64];
    let mut needed = 0;
    if unsafe {
        GetUserObjectInformationW(
            desktop.0,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            std::mem::size_of_val(&name) as u32,
            &mut needed,
        )
    } == 0
        || needed == 0
        || needed as usize > std::mem::size_of_val(&name)
    {
        return Err(error("TARGET_UNAVAILABLE"));
    }
    let end = name
        .iter()
        .position(|value| *value == 0)
        .ok_or_else(|| error("TARGET_UNAVAILABLE"))?;
    if String::from_utf16(&name[..end]).map_err(|_| error("TARGET_UNAVAILABLE"))? != "Default" {
        return Err(error("TARGET_UNAVAILABLE"));
    }
    Ok(())
}

fn valid_session_info(info: &WTSINFOEXW, expected_session: u32) -> bool {
    if info.Level != 1 {
        return false;
    }
    let state = unsafe { info.Data.WTSInfoExLevel1 };
    state.SessionId == expected_session
        && state.SessionState == WTSActive
        && state.SessionFlags == WTS_SESSIONSTATE_UNLOCK as i32
}

fn complete_guard(target: &CaptureTarget) -> Result<(), DesktopAdapterError> {
    scalar_guard(target)?;
    let current = capture_target_probe(&target.executable_path, &|| false, Duration::from_secs(2))?;
    if !current.same_identity(target) {
        return Err(error("TARGET_CHANGED"));
    }
    scalar_guard(target)
}

struct Pixels {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}
type FrameResult = Arc<Mutex<Option<Result<Pixels, DesktopAdapterError>>>>;
struct Flags {
    target: CaptureTarget,
    // The library owns real frame.ContentSize matching/recreation. This also
    // checks the public capture-item size against the actual surface dimensions.
    item_size: Arc<dyn Fn() -> Result<(u32, u32), String> + Send + Sync>,
    result: FrameResult,
}
struct OneFrame(Flags);

impl GraphicsCaptureApiHandler for OneFrame {
    type Flags = Flags;
    type Error = String;
    fn new(context: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self(context.flags))
    }
    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), String> {
        let mut result = self
            .0
            .result
            .lock()
            .map_err(|_| "ADAPTER_FAILED".to_string())?;
        if result.is_none() {
            *result = Some((|| {
                scalar_guard(&self.0.target)?;
                let (width, height) = (frame.width(), frame.height());
                let content = (self.0.item_size)().map_err(|_| error("TARGET_UNAVAILABLE"))?;
                validate_frame_size((width, height), content)?;
                if frame.color_format() != ColorFormat::Rgba8 {
                    return Err(error("TARGET_CHANGED"));
                }
                let mut buffer = frame.buffer().map_err(|_| error("TARGET_UNAVAILABLE"))?;
                let pitch = buffer.row_pitch() as usize;
                if buffer.width() != width
                    || buffer.height() != height
                    || buffer.color_format() != ColorFormat::Rgba8
                    || pitch > 8192 * 4 + 4096
                {
                    return Err(error("SCAN_LIMIT"));
                }
                let pixels = copy_rgba_rows(buffer.as_raw_buffer(), width, height, pitch)?;
                validate_visible_pixels(&pixels)?;
                scalar_guard(&self.0.target)?;
                Ok(Pixels {
                    width,
                    height,
                    rgba: pixels,
                })
            })());
        }
        // Only memory has changed. No stdout output until stop/join AND final guard.
        control.stop();
        Ok(())
    }
    fn on_closed(&mut self) -> Result<(), String> {
        let mut result = self
            .0
            .result
            .lock()
            .map_err(|_| "ADAPTER_FAILED".to_string())?;
        if result.is_none() {
            *result = Some(Err(error("TARGET_CHANGED")));
        }
        Ok(())
    }
}

pub(super) fn capture(target: CaptureTarget) -> Result<Vec<u8>, DesktopAdapterError> {
    target.validate()?;
    complete_guard(&target)?;
    let _runtime = WinRt::new()?;
    if !GraphicsCaptureApi::is_supported().map_err(|_| error("UNSUPPORTED_PLATFORM"))? {
        return Err(error("UNSUPPORTED_PLATFORM"));
    }
    let secondary = if GraphicsCaptureApi::is_secondary_windows_supported()
        .map_err(|_| error("UNSUPPORTED_PLATFORM"))?
    {
        SecondaryWindowSettings::Exclude
    } else {
        SecondaryWindowSettings::Default
    };
    let cursor = if GraphicsCaptureApi::is_cursor_settings_supported()
        .map_err(|_| error("UNSUPPORTED_PLATFORM"))?
    {
        CursorCaptureSettings::WithoutCursor
    } else {
        CursorCaptureSettings::Default
    };
    let window = Window::from_raw_hwnd(target.window_handle as usize as *mut std::ffi::c_void);
    let item: GraphicsCaptureItemType =
        window.try_into().map_err(|_| error("TARGET_UNAVAILABLE"))?;
    let GraphicsCaptureItemType::Window((ref native_item, _)) = item else {
        return Err(error("TARGET_CHANGED"));
    };
    let native_item = native_item.clone();
    let size = native_item
        .Size()
        .map_err(|_| error("TARGET_UNAVAILABLE"))?;
    if size.Width <= 0 || size.Height <= 0 {
        return Err(error("TARGET_UNAVAILABLE"));
    }
    validate_source_dimensions(size.Width as u32, size.Height as u32)?;
    let size_probe = Arc::new(move || {
        let size = native_item
            .Size()
            .map_err(|_| "TARGET_UNAVAILABLE".to_string())?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err("TARGET_UNAVAILABLE".into());
        }
        Ok((size.Width as u32, size.Height as u32))
    });
    let result: FrameResult = Arc::new(Mutex::new(None));
    let settings = Settings::new(
        item,
        cursor,
        DrawBorderSettings::Default,
        secondary,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        Flags {
            target: target.clone(),
            item_size: size_probe,
            result: Arc::clone(&result),
        },
    );
    // Library init/message loop/join may block. The parent deadline kills/reaps
    // this helper and its UIA probe descendants; dropping a future is not cleanup.
    OneFrame::start(settings).map_err(|_| error("TARGET_UNAVAILABLE"))?;
    let pixels = result
        .lock()
        .map_err(|_| error("ADAPTER_FAILED"))?
        .take()
        .ok_or_else(|| error("TARGET_UNAVAILABLE"))??;
    complete_guard(&target)?;
    let png = encode_rgba(pixels.width, pixels.height, pixels.rgba)?;
    scalar_guard(&target)?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_locked_disconnected_or_different_sessions_deny_without_os_queries() {
        use windows_sys::Win32::System::RemoteDesktop::WTSINFOEX_LEVEL1_W;
        let make = |level, session, state, flags| {
            let mut info = WTSINFOEXW::default();
            info.Level = level;
            let mut data = WTSINFOEX_LEVEL1_W::default();
            data.SessionId = session;
            data.SessionState = state;
            data.SessionFlags = flags;
            info.Data.WTSInfoExLevel1 = data;
            info
        };
        let unlocked = WTS_SESSIONSTATE_UNLOCK as i32;
        let info = make(1, 2, WTSActive, unlocked);
        assert!(valid_session_info(&info, 2));
        assert!(!valid_session_info(&info, 3));
        for flags in [-1, 0, 2] {
            assert!(!valid_session_info(&make(1, 2, WTSActive, flags), 2));
        }
        assert!(!valid_session_info(&make(1, 2, 4, unlocked), 2)); // WTSDisconnected.
        assert!(!valid_session_info(&make(0, 2, WTSActive, unlocked), 2));
    }
}
