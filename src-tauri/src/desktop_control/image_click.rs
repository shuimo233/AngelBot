//! Bounded image observations and one-shot clicks. No poller or stored pixels.
use super::{DesktopAdapterError, DesktopWindowImage};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopImageReference {
    pub image_ref: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DesktopImageClickRequest {
    pub app_id: String,
    pub image_ref: String,
    pub x: u32,
    pub y: u32,
}

impl DesktopImageClickRequest {
    pub(crate) fn validate(&self) -> Result<(), DesktopAdapterError> {
        if self.image_ref.is_empty()
            || self.image_ref.len() > 128
            || self.x >= 2048
            || self.y >= 2048
        {
            return Err(DesktopAdapterError::new(
                "INVALID_ARGUMENT",
                "Use an observed image reference and in-image integer coordinates",
            ));
        }
        Ok(())
    }
}

/// Host-only binding, never deserialized from model input or persisted.
#[derive(Clone)]
pub struct DesktopImageClickTarget {
    request: DesktopImageClickRequest,
    #[cfg(windows)]
    window: crate::window_capture::CaptureTarget,
    #[cfg(windows)]
    bounds: [i32; 4],
    #[cfg(windows)]
    digest: [u8; 32],
    #[cfg(windows)]
    client: [i32; 4],
    expires_at: Instant,
}

impl DesktopImageClickTarget {
    #[cfg(test)]
    pub(crate) fn fixture(request: DesktopImageClickRequest) -> Self {
        Self {
            request,
            expires_at: Instant::now() + Duration::from_secs(60),
            #[cfg(windows)]
            window: crate::window_capture::CaptureTarget {
                process_id: 0,
                window_handle: 0,
                executable_path: "fixture-only".into(),
            },
            #[cfg(windows)]
            bounds: [0, 0, 1, 1],
            #[cfg(windows)]
            client: [0, 0, 1, 1],
            #[cfg(windows)]
            digest: [0; 32],
        }
    }
    pub(crate) fn matches(&self, request: &DesktopImageClickRequest) -> bool {
        &self.request == request && self.expires_at > Instant::now()
    }
    pub(crate) fn remaining(&self) -> Duration {
        self.expires_at.saturating_duration_since(Instant::now())
    }
    #[cfg(windows)]
    pub(crate) fn width(&self) -> u32 {
        (self.bounds[2] - self.bounds[0]) as u32
    }
    #[cfg(windows)]
    pub(crate) fn height(&self) -> u32 {
        (self.bounds[3] - self.bounds[1]) as u32
    }
    #[cfg(windows)]
    pub(crate) fn same_observation(&self, other: &Self) -> bool {
        self.request == other.request
            && self.window.same_identity(&other.window)
            && self.bounds == other.bounds
            && self.client == other.client
            && self.digest == other.digest
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopImageClickDisplay {
    pub image_data_url: String,
    pub width: u32,
    pub height: u32,
    pub x: u32,
    pub y: u32,
}
impl std::fmt::Debug for DesktopImageClickDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DesktopImageClickDisplay")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("x", &self.x)
            .field("y", &self.y)
            .finish_non_exhaustive()
    }
}
pub struct DesktopImageClickPreview {
    pub image: DesktopImageClickDisplay,
    pub target: DesktopImageClickTarget,
}

#[cfg(windows)]
pub(super) use native::*;

#[cfg(windows)]
mod native {
    use super::*;
    use crate::window_capture::{error, CaptureTarget};
    use sha2::{Digest, Sha256};
    use std::{collections::HashMap, sync::Mutex};
    use windows_sys::Win32::{
        Foundation::{POINT, RECT},
        Graphics::{
            Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
            Gdi::ClientToScreen,
        },
        UI::{
            HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE,
                MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE,
                MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT,
            },
            WindowsAndMessaging::{
                GetAncestor, GetClientRect, GetForegroundWindow, GetGUIThreadInfo,
                GetSystemMetrics, GetWindowThreadProcessId, SetForegroundWindow, WindowFromPoint,
                GA_ROOT, GUITHREADINFO, GUI_INMENUMODE, GUI_INMOVESIZE, GUI_POPUPMENUMODE,
                GUI_SYSTEMMENUMODE, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
                SM_YVIRTUALSCREEN,
            },
        },
    };

    struct PhysicalDpi(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT);
    impl PhysicalDpi {
        fn enter() -> Result<Self, DesktopAdapterError> {
            let previous =
                unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
            if previous.is_null() {
                return Err(error("TARGET_UNAVAILABLE"));
            }
            Ok(Self(previous))
        }
    }
    impl Drop for PhysicalDpi {
        fn drop(&mut self) {
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }

    const TTL: Duration = Duration::from_secs(300);
    #[derive(Default)]
    pub(crate) struct ImageReferenceStore(Mutex<HashMap<String, DesktopImageClickTarget>>);
    impl ImageReferenceStore {
        pub(crate) fn issue(
            &self,
            app: &str,
            window: CaptureTarget,
            bounds: [i32; 4],
            png: &[u8],
        ) -> Result<Option<DesktopImageReference>, DesktopAdapterError> {
            let (width, height) = png_dimensions(png)?;
            // Never guess WGC/DWM scaling, borders or DPI transforms. Large
            // resized images remain readable, but cannot mint click proposals.
            if width != (bounds[2] - bounds[0]) as u32 || height != (bounds[3] - bounds[1]) as u32 {
                return Ok(None);
            }
            let mut leases = self.0.lock().map_err(|_| error("TARGET_UNAVAILABLE"))?;
            leases.retain(|_, v| v.expires_at > Instant::now() && v.request.app_id != app);
            if leases.len() >= 8 {
                return Ok(None);
            }
            let id = uuid::Uuid::new_v4().to_string();
            let Ok(client) = client_bounds(&window) else {
                return Ok(None);
            };
            let digest = match client_digest(png, bounds, client) {
                Ok(digest) => digest,
                Err(error) if error.code == "FRAME_CHANGED" => return Ok(None),
                Err(error) => return Err(error),
            };
            leases.insert(
                id.clone(),
                DesktopImageClickTarget {
                    request: DesktopImageClickRequest {
                        app_id: app.into(),
                        image_ref: id.clone(),
                        x: 0,
                        y: 0,
                    },
                    window,
                    bounds,
                    client,
                    digest,
                    expires_at: Instant::now() + TTL,
                },
            );
            Ok(Some(DesktopImageReference {
                image_ref: id,
                width,
                height,
            }))
        }
        pub(crate) fn resolve(
            &self,
            req: &DesktopImageClickRequest,
            executable: &str,
        ) -> Result<DesktopImageClickTarget, DesktopAdapterError> {
            req.validate()?;
            let mut leases = self.0.lock().map_err(|_| error("TARGET_UNAVAILABLE"))?;
            leases.retain(|_, v| v.expires_at > Instant::now());
            let mut target = leases
                .get(&req.image_ref)
                .cloned()
                .ok_or_else(|| error("TARGET_CHANGED"))?;
            if target.request.app_id != req.app_id
                || !target
                    .window
                    .executable_path
                    .eq_ignore_ascii_case(executable)
                || req.x >= target.width()
                || req.y >= target.height()
            {
                return Err(error("TARGET_CHANGED"));
            }
            target.request = req.clone();
            Ok(target)
        }
        pub(crate) fn revoke(&self, app: &str) -> Result<(), DesktopAdapterError> {
            self.0
                .lock()
                .map_err(|_| error("TARGET_UNAVAILABLE"))?
                .retain(|_, v| v.request.app_id != app);
            Ok(())
        }
    }

    fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32), DesktopAdapterError> {
        let mut decoder = png::Decoder::new(bytes);
        decoder.set_limits(png::Limits {
            bytes: 16 * 1024 * 1024,
        });
        let reader = decoder.read_info().map_err(|_| error("CAPTURE_FAILED"))?;
        let (w, h) = (reader.info().width, reader.info().height);
        if w == 0 || h == 0 || w > 2048 || h > 2048 {
            return Err(error("CAPTURE_LIMIT"));
        }
        Ok((w, h))
    }
    pub(crate) fn window_bounds(window: &CaptureTarget) -> Result<[i32; 4], DesktopAdapterError> {
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        if unsafe {
            DwmGetWindowAttribute(
                window.window_handle as _,
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                (&mut rect as *mut RECT).cast(),
                std::mem::size_of::<RECT>() as u32,
            )
        } < 0
            || !(1..=8192).contains(&(i64::from(rect.right) - i64::from(rect.left)))
            || !(1..=8192).contains(&(i64::from(rect.bottom) - i64::from(rect.top)))
        {
            return Err(error("TARGET_UNAVAILABLE"));
        }
        Ok([rect.left, rect.top, rect.right, rect.bottom])
    }
    fn client_bounds(window: &CaptureTarget) -> Result<[i32; 4], DesktopAdapterError> {
        let _dpi = PhysicalDpi::enter()?;
        let mut client: RECT = unsafe { std::mem::zeroed() };
        let mut origin = POINT { x: 0, y: 0 };
        if unsafe { GetClientRect(window.window_handle as _, &mut client) } == 0
            || unsafe { ClientToScreen(window.window_handle as _, &mut origin) } == 0
            || client.right <= 0
            || client.bottom <= 0
        {
            return Err(error("TARGET_UNAVAILABLE"));
        }
        Ok([
            origin.x,
            origin.y,
            origin
                .x
                .checked_add(client.right)
                .ok_or_else(|| error("TARGET_CHANGED"))?,
            origin
                .y
                .checked_add(client.bottom)
                .ok_or_else(|| error("TARGET_CHANGED"))?,
        ])
    }
    fn client_digest(
        png: &[u8],
        bounds: [i32; 4],
        client: [i32; 4],
    ) -> Result<[u8; 32], DesktopAdapterError> {
        let mut decoder = png::Decoder::new(png);
        decoder.set_limits(png::Limits {
            bytes: 16 * 1024 * 1024,
        });
        let mut reader = decoder.read_info().map_err(|_| error("CAPTURE_FAILED"))?;
        if reader.info().width > 2048
            || reader.info().height > 2048
            || reader.output_buffer_size() > 16 * 1024 * 1024
        {
            return Err(error("CAPTURE_LIMIT"));
        }
        let mut pixels = vec![0; reader.output_buffer_size()];
        let frame = reader
            .next_frame(&mut pixels)
            .map_err(|_| error("CAPTURE_FAILED"))?;
        if client[2] <= client[0]
            || client[3] <= client[1]
            || frame.color_type != png::ColorType::Rgba
            || frame.bit_depth != png::BitDepth::Eight
            || i64::from(frame.width) != i64::from(bounds[2]) - i64::from(bounds[0])
            || i64::from(frame.height) != i64::from(bounds[3]) - i64::from(bounds[1])
            || client[0] < bounds[0]
            || client[1] < bounds[1]
            || client[2] > bounds[2]
            || client[3] > bounds[3]
        {
            return Err(error("FRAME_CHANGED"));
        }
        let mut hash = Sha256::new();
        let left = (client[0] - bounds[0]) as usize;
        let right = (client[2] - bounds[0]) as usize;
        for row in (client[1] - bounds[1]) as usize..(client[3] - bounds[1]) as usize {
            hash.update(
                &pixels[(row * frame.width as usize + left) * 4
                    ..(row * frame.width as usize + right) * 4],
            );
        }
        Ok(hash.finalize().into())
    }
    fn point(target: &DesktopImageClickTarget) -> Result<POINT, DesktopAdapterError> {
        let x = target.bounds[0]
            .checked_add(target.request.x as i32)
            .ok_or_else(|| error("TARGET_CHANGED"))?;
        let y = target.bounds[1]
            .checked_add(target.request.y as i32)
            .ok_or_else(|| error("TARGET_CHANGED"))?;
        let client = client_bounds(&target.window)?;
        if client != target.client
            || x < client[0]
            || y < client[1]
            || x >= client[2]
            || y >= client[3]
        {
            return Err(error("TARGET_CHANGED"));
        }
        Ok(POINT { x, y })
    }
    pub(crate) fn recheck(
        target: &DesktopImageClickTarget,
        cancel: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<DesktopWindowImage, DesktopAdapterError> {
        if cancel() || target.remaining().is_zero() || timeout.is_zero() {
            return Err(error("SCAN_CANCELLED"));
        }
        let started = Instant::now();
        let current = crate::desktop_control::capture_target_probe(
            &target.window.executable_path,
            cancel,
            timeout,
        )?;
        if !current.same_identity(&target.window) || window_bounds(&current)? != target.bounds {
            return Err(error("TARGET_CHANGED"));
        }
        point(target)?;
        let image = crate::window_capture::capture_with_control(
            &current,
            cancel,
            timeout.saturating_sub(started.elapsed()),
        )?;
        if cancel() || target.remaining().is_zero() {
            return Err(error("SCAN_CANCELLED"));
        }
        let hash = client_digest(image.png_bytes(), target.bounds, target.client)?;
        if hash != target.digest || window_bounds(&current)? != target.bounds {
            return Err(error("FRAME_CHANGED"));
        }
        Ok(image)
    }
    fn no_held_input() -> bool {
        // Include ordinary keys and every mouse button, not just modifiers:
        // a held Enter/key repeat must not follow the approval focus handoff.
        // Only the current-down high bit is meaningful; ignore the race-prone
        // "pressed since last query" bit. Never synthesize releases for users.
        (1..=254).all(|key| unsafe { GetAsyncKeyState(key) } >= 0)
    }
    fn ordinary_input_state(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
        let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        let thread = unsafe { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) };
        thread != 0
            && unsafe { GetGUIThreadInfo(thread, &mut info) } != 0
            && info.hwndCapture.is_null()
            && info.flags
                & (GUI_INMENUMODE | GUI_INMOVESIZE | GUI_POPUPMENUMODE | GUI_SYSTEMMENUMODE)
                == 0
    }
    pub(crate) fn click(
        target: &DesktopImageClickTarget,
        cancel: &dyn Fn() -> bool,
        timeout: Duration,
    ) -> Result<(), DesktopAdapterError> {
        let _dpi = PhysicalDpi::enter()?;
        let hwnd = target.window.window_handle as _;
        if cancel() || !target.matches(&target.request) {
            return Err(error("SCAN_CANCELLED"));
        }
        if !no_held_input() || !ordinary_input_state(hwnd) {
            return Err(error("TARGET_UNAVAILABLE"));
        }
        let foreground = unsafe { GetForegroundWindow() };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(foreground, &mut pid) };
        if foreground != hwnd && pid != std::process::id() {
            return Err(error("TARGET_CHANGED"));
        }
        if foreground != hwnd && unsafe { SetForegroundWindow(hwnd) } == 0 {
            return Err(error("TARGET_UNAVAILABLE"));
        }
        // Focus may change client content. Re-observe after the handoff before
        // any click, rather than trusting a frame from before confirmation.
        recheck(target, cancel, timeout)?;
        let p = point(target)?;
        if cancel()
            || window_bounds(&target.window)? != target.bounds
            || unsafe { GetForegroundWindow() } != hwnd
            || !no_held_input()
            || !ordinary_input_state(hwnd)
        {
            return Err(error("TARGET_CHANGED"));
        }
        if cancel()
            || !no_held_input()
            || !ordinary_input_state(hwnd)
            || unsafe { GetForegroundWindow() } != hwnd
            || unsafe { GetAncestor(WindowFromPoint(p), GA_ROOT) } != hwnd
        {
            return Err(error("TARGET_CHANGED"));
        }
        let inputs = click_inputs(
            p,
            [
                unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) },
                unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) },
                unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) },
                unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) },
            ],
        )?;
        // MOVE/DOWN/UP are submitted as one ordered input array. No separate
        // cursor move leaves a user-movement gap before the button events.
        let sent = unsafe { SendInput(3, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
        if sent != 3 {
            // Only if our DOWN was inserted do we release our pressed button.
            // A release is cleanup, never a retry or proof of business success.
            if sent == 2 {
                let release = mouse(0, 0, MOUSEEVENTF_LEFTUP);
                unsafe { SendInput(1, &release, std::mem::size_of::<INPUT>() as i32) };
            }
            return Err(DesktopAdapterError::new(
                "RESULT_UNKNOWN",
                "Input dispatch could not be confirmed; inspect the app, do not replay",
            ));
        }
        Ok(())
    }

    fn mouse(dx: i32, dy: i32, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }
    fn click_inputs(point: POINT, desktop: [i32; 4]) -> Result<[INPUT; 3], DesktopAdapterError> {
        let [left, top, width, height] = desktop;
        let x = i64::from(point.x) - i64::from(left);
        let y = i64::from(point.y) - i64::from(top);
        if !(2..=65536).contains(&width)
            || !(2..=65536).contains(&height)
            || x < 0
            || y < 0
            || x >= i64::from(width)
            || y >= i64::from(height)
        {
            return Err(error("TARGET_CHANGED"));
        }
        let nx = ((x * 65535 + i64::from(width - 1) / 2) / i64::from(width - 1)) as i32;
        let ny = ((y * 65535 + i64::from(height - 1) / 2) / i64::from(height - 1)) as i32;
        Ok([
            mouse(
                nx,
                ny,
                MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            ),
            mouse(0, 0, MOUSEEVENTF_LEFTDOWN),
            mouse(0, 0, MOUSEEVENTF_LEFTUP),
        ])
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        fn png(pixels: &[u8]) -> Vec<u8> {
            let mut bytes = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut bytes, 4, 4);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().unwrap();
                writer.write_image_data(pixels).unwrap();
            }
            bytes
        }
        #[test]
        fn client_hash_ignores_caption_but_rejects_content_geometry_and_oversized_decode() {
            let mut pixels = vec![40; 4 * 4 * 4];
            let baseline =
                client_digest(&png(&pixels), [100, 100, 104, 104], [101, 101, 103, 103]).unwrap();
            pixels[0] = 90;
            assert_eq!(
                client_digest(&png(&pixels), [100, 100, 104, 104], [101, 101, 103, 103]).unwrap(),
                baseline
            );
            pixels[(4 + 1) * 4] = 90;
            assert_ne!(
                client_digest(&png(&pixels), [100, 100, 104, 104], [101, 101, 103, 103]).unwrap(),
                baseline
            );
            for client in [[99, 100, 103, 103], [101, 101, 105, 103]] {
                assert!(client_digest(&png(&pixels), [100, 100, 104, 104], client).is_err());
            }
            assert!(client_digest(b"not png", [0, 0, 4, 4], [0, 0, 4, 4]).is_err());
        }
        #[test]
        fn observation_binding_is_expiring_app_scoped_bounded_and_revocable_without_pixels() {
            let req = DesktopImageClickRequest {
                app_id: "a".into(),
                image_ref: "source".into(),
                x: 0,
                y: 0,
            };
            let mut target = DesktopImageClickTarget::fixture(req.clone());
            assert!(target.matches(&req));
            target.window.executable_path = "C:\\fixture.exe".into();
            let store = ImageReferenceStore::default();
            store
                .0
                .lock()
                .unwrap()
                .insert(req.image_ref.clone(), target.clone());
            assert!(store.resolve(&req, "c:\\FIXTURE.exe").is_ok());
            for changed in [
                DesktopImageClickRequest {
                    app_id: "b".into(),
                    ..req.clone()
                },
                DesktopImageClickRequest {
                    x: 1,
                    ..req.clone()
                },
                DesktopImageClickRequest {
                    image_ref: "forged".into(),
                    ..req.clone()
                },
            ] {
                assert!(store.resolve(&changed, "C:\\fixture.exe").is_err());
            }
            assert!(store.resolve(&req, "C:\\other.exe").is_err());
            store
                .0
                .lock()
                .unwrap()
                .get_mut("source")
                .unwrap()
                .expires_at = Instant::now();
            assert!(store.resolve(&req, "C:\\fixture.exe").is_err());
            store.0.lock().unwrap().insert("source".into(), target);
            store.revoke("a").unwrap();
            assert!(store.0.lock().unwrap().is_empty());
        }
        #[test]
        fn input_batch_maps_virtual_desktop_and_contains_only_move_down_up() {
            for (p, expected) in [
                (POINT { x: -100, y: 50 }, (0, 0)),
                (POINT { x: 99, y: 149 }, (65535, 65535)),
            ] {
                let inputs = click_inputs(p, [-100, 50, 200, 100]).unwrap();
                let movement = unsafe { inputs[0].Anonymous.mi };
                assert_eq!((movement.dx, movement.dy), expected);
                assert_eq!(
                    movement.dwFlags,
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK
                );
                assert_eq!(
                    unsafe { inputs[1].Anonymous.mi }.dwFlags,
                    MOUSEEVENTF_LEFTDOWN
                );
                assert_eq!(
                    unsafe { inputs[2].Anonymous.mi }.dwFlags,
                    MOUSEEVENTF_LEFTUP
                );
            }
            for bounds in [[0, 0, 1, 100], [0, 0, 70000, 100], [0, 0, 100, 0]] {
                assert!(click_inputs(POINT { x: 0, y: 0 }, bounds).is_err());
            }
            assert!(click_inputs(POINT { x: 100, y: 0 }, [0, 0, 100, 100]).is_err());
        }
    }
}
