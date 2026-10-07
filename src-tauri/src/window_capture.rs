//! One-shot window capture. The host owns cancellation, output budgets and reaping.
//! No model types, image serialization, user data stores, or disk images belong here.

use crate::desktop_control::{DesktopAdapterError, DesktopWindowImage};
use serde::{Deserialize, Serialize};
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(windows)]
mod native;

const HELPER_ARGUMENT: &str = "--angelbot-window-capture-helper-v1";
const MAX_TARGET_BYTES: usize = 8192;
pub(crate) const MAX_PNG_BYTES: usize = 2 * 1024 * 1024;
const MAX_SOURCE_EDGE: u32 = 8192;
const MAX_SOURCE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_IMAGE_EDGE: u32 = 2048;
const MAX_IMAGE_PIXELS: u64 = 4 * 1024 * 1024;

// These are host-issued identities, not selectors accepted from a model.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureTarget {
    pub process_id: u32,
    pub window_handle: u64,
    pub executable_path: String,
}

impl CaptureTarget {
    fn validate(&self) -> Result<(), DesktopAdapterError> {
        if self.process_id == 0
            || self.window_handle == 0
            || self.window_handle > isize::MAX as u64
            || self.executable_path.is_empty()
            || self.executable_path.len() > 4096
            || self.executable_path.chars().any(char::is_control)
            || !std::path::Path::new(&self.executable_path).is_absolute()
        {
            return Err(error("TARGET_CHANGED"));
        }
        Ok(())
    }

    pub(crate) fn same_identity(&self, other: &Self) -> bool {
        self.process_id == other.process_id
            && self.window_handle == other.window_handle
            && self
                .executable_path
                .eq_ignore_ascii_case(&other.executable_path)
    }
}

pub(crate) fn error(code: &'static str) -> DesktopAdapterError {
    let message = match code {
        "SCAN_CANCELLED" => "Single-window observation was cancelled",
        "SCAN_TIMEOUT" => "Single-window observation exceeded its time budget",
        "SCAN_LIMIT" => "Single-window observation exceeded its data budget",
        "TARGET_CHANGED" => "The trusted application window changed",
        "TARGET_NOT_FOUND" => "The trusted application has no open main window",
        "TARGET_AMBIGUOUS" => "The trusted application has multiple main windows",
        "TARGET_UNAVAILABLE" => "The trusted window or its safety state is unavailable",
        "SENSITIVE_SURFACE" => "A password or protected window cannot be observed as an image",
        "UNSUPPORTED_PLATFORM" => "Windows Graphics Capture is unavailable",
        _ => "The single-window capture helper failed; no image was returned",
    };
    DesktopAdapterError::new(code, message)
}

pub(crate) fn parse_target(bytes: &[u8]) -> Result<CaptureTarget, DesktopAdapterError> {
    if bytes.len() > MAX_TARGET_BYTES {
        return Err(error("SCAN_LIMIT"));
    }
    let target: CaptureTarget =
        serde_json::from_slice(bytes).map_err(|_| error("TARGET_CHANGED"))?;
    target.validate()?;
    Ok(target)
}

/// Called before Tauri, the app DB, credentials, logging or panic diagnostics.
/// An explicit hidden helper argument is the only route into this mode.
pub fn helper_exit_code() -> Option<i32> {
    if !helper_arguments(std::env::args_os().skip(1))? {
        return Some(2);
    }
    std::panic::set_hook(Box::new(|_| {}));
    #[cfg(windows)]
    {
        use std::io::Write;
        let result = std::panic::catch_unwind(|| {
            let mut bytes = Vec::new();
            io::stdin()
                .take((MAX_TARGET_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| error("TARGET_CHANGED"))?;
            let target = parse_target(&bytes)?;
            let png = native::capture(target)?;
            validate_png(&png)?;
            io::stdout()
                .write_all(&png)
                .map_err(|_| error("ADAPTER_FAILED"))?;
            io::stdout().flush().map_err(|_| error("ADAPTER_FAILED"))
        });
        match result {
            Ok(Ok(())) => Some(0),
            Ok(Err(failure)) => {
                // Only fixed allowlisted categories, never OS/UIA errors or pixels.
                let code = match failure.code.as_str() {
                    "SCAN_LIMIT"
                    | "SCAN_TIMEOUT"
                    | "TARGET_NOT_FOUND"
                    | "TARGET_AMBIGUOUS"
                    | "TARGET_CHANGED"
                    | "TARGET_UNAVAILABLE"
                    | "SENSITIVE_SURFACE"
                    | "UNSUPPORTED_PLATFORM" => failure.code.as_str(),
                    _ => "ADAPTER_FAILED",
                };
                let _ = writeln!(io::stderr(), "{code}");
                Some(2)
            }
            Err(_) => Some(2),
        }
    }
    #[cfg(not(windows))]
    Some(2)
}

fn helper_arguments(mut args: impl Iterator<Item = std::ffi::OsString>) -> Option<bool> {
    if args.next().as_deref() != Some(std::ffi::OsStr::new(HELPER_ARGUMENT)) {
        return None;
    }
    Some(args.next().is_none())
}

#[cfg(windows)]
pub(crate) fn capture_with_control(
    target: &CaptureTarget,
    is_cancelled: &dyn Fn() -> bool,
    timeout: std::time::Duration,
) -> Result<DesktopWindowImage, DesktopAdapterError> {
    use std::process::{Command, Stdio};
    target.validate()?;
    let payload = serde_json::to_vec(target).map_err(|_| error("TARGET_CHANGED"))?;
    if payload.len() > MAX_TARGET_BYTES {
        return Err(error("SCAN_LIMIT"));
    }
    let executable = std::env::current_exe().map_err(|_| error("ADAPTER_FAILED"))?;
    let mut command = Command::new(executable);
    crate::child_process_env::apply_minimal_child_environment(&mut command);
    command
        .arg(HELPER_ARGUMENT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = run_bounded_child(
        &mut command,
        Some(payload),
        MAX_PNG_BYTES,
        is_cancelled,
        timeout,
    )?;
    if !output.status.success() {
        return Err(helper_failure(&output.stderr));
    }
    if is_cancelled() {
        return Err(error("SCAN_CANCELLED"));
    }
    validate_png(&output.stdout)?;
    Ok(DesktopWindowImage::from_png_bytes(output.stdout))
}

#[cfg(windows)]
fn helper_failure(stderr: &[u8]) -> DesktopAdapterError {
    match std::str::from_utf8(stderr).unwrap_or("").trim() {
        "SCAN_LIMIT" => error("SCAN_LIMIT"),
        "SCAN_TIMEOUT" => error("SCAN_TIMEOUT"),
        "TARGET_NOT_FOUND" => error("TARGET_NOT_FOUND"),
        "TARGET_AMBIGUOUS" => error("TARGET_AMBIGUOUS"),
        "TARGET_CHANGED" => error("TARGET_CHANGED"),
        "TARGET_UNAVAILABLE" => error("TARGET_UNAVAILABLE"),
        "SENSITIVE_SURFACE" => error("SENSITIVE_SURFACE"),
        "UNSUPPORTED_PLATFORM" => error("UNSUPPORTED_PLATFORM"),
        _ => error("ADAPTER_FAILED"),
    }
}

#[cfg(windows)]
pub(crate) struct BoundedChildOutput {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

fn drain_bounded(
    mut reader: impl Read,
    limit: usize,
    exceeded: &AtomicBool,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let read = match reader.read(&mut chunk) {
            Err(failure) if failure.kind() == io::ErrorKind::Interrupted => continue,
            other => other?,
        };
        if read == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(read) > limit {
            exceeded.store(true, Ordering::Release);
            // Continue discarding until the host terminates/reaps the process tree.
        } else if !exceeded.load(Ordering::Acquire) {
            bytes.extend_from_slice(&chunk[..read]);
        }
    }
}

/// Concurrent bounded pipe draining is essential: MiB images cannot wait for
/// process exit before stdout is read. The target is written only AFTER job assignment.
#[cfg(windows)]
pub(crate) fn run_bounded_child(
    command: &mut std::process::Command,
    input: Option<Vec<u8>>,
    stdout_limit: usize,
    is_cancelled: &dyn Fn() -> bool,
    timeout: std::time::Duration,
) -> Result<BoundedChildOutput, DesktopAdapterError> {
    use crate::child_process_tree::ProcessTree;
    use std::io::Write;
    use std::time::{Duration, Instant};
    if is_cancelled() {
        return Err(error("SCAN_CANCELLED"));
    }
    if timeout.is_zero() {
        return Err(error("SCAN_TIMEOUT"));
    }
    struct ReapOnDrop(ProcessTree);
    impl Drop for ReapOnDrop {
        fn drop(&mut self) {
            let _ = self.0.terminate_and_wait();
        }
    }
    let started = Instant::now();
    let mut child = ReapOnDrop(ProcessTree::spawn(command).map_err(|_| error("ADAPTER_FAILED"))?);
    let stdout = child
        .0
        .take_stdout()
        .ok_or_else(|| error("ADAPTER_FAILED"))?;
    let stderr = child
        .0
        .take_stderr()
        .ok_or_else(|| error("ADAPTER_FAILED"))?;
    let exceeded = Arc::new(AtomicBool::new(false));
    let out_signal = Arc::clone(&exceeded);
    let err_signal = Arc::clone(&exceeded);
    let out_thread = std::thread::spawn(move || drain_bounded(stdout, stdout_limit, &out_signal));
    let err_thread = std::thread::spawn(move || drain_bounded(stderr, 8192, &err_signal));
    let input_thread = input.map(|bytes| {
        let stdin = child.0.take_stdin();
        std::thread::spawn(move || {
            stdin
                .ok_or_else(|| io::Error::other("missing helper input"))?
                .write_all(&bytes)
        })
    });
    let outcome = loop {
        if is_cancelled() {
            break Err(error("SCAN_CANCELLED"));
        }
        if started.elapsed() >= timeout {
            break Err(error("SCAN_TIMEOUT"));
        }
        if exceeded.load(Ordering::Acquire) {
            break Err(error("SCAN_LIMIT"));
        }
        match child.0.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break Err(error("ADAPTER_FAILED")),
        }
    };
    // Also end descendants that kept stdout open after an early root exit.
    let cleanup = child.0.terminate_and_wait();
    drop(child); // Closing the job is a second cleanup boundary if termination failed.
    let stdout = out_thread.join().map_err(|_| error("ADAPTER_FAILED"))?;
    let stderr = err_thread.join().map_err(|_| error("ADAPTER_FAILED"))?;
    let written = input_thread.map(|thread| thread.join());
    cleanup.map_err(|_| error("ADAPTER_FAILED"))?;
    let status = outcome?;
    if is_cancelled() {
        return Err(error("SCAN_CANCELLED"));
    }
    if exceeded.load(Ordering::Acquire) {
        return Err(error("SCAN_LIMIT"));
    }
    if written.is_some_and(|result| !matches!(result, Ok(Ok(())))) {
        return Err(error("ADAPTER_FAILED"));
    }
    Ok(BoundedChildOutput {
        status,
        stdout: stdout.map_err(|_| error("ADAPTER_FAILED"))?,
        stderr: stderr.map_err(|_| error("ADAPTER_FAILED"))?,
    })
}

fn validate_source_dimensions(width: u32, height: u32) -> Result<usize, DesktopAdapterError> {
    if width == 0
        || height == 0
        || width > MAX_SOURCE_EDGE
        || height > MAX_SOURCE_EDGE
        || u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS
    {
        return Err(error("SCAN_LIMIT"));
    }
    Ok(width as usize * height as usize * 4)
}

pub(crate) fn validate_frame_size(
    surface: (u32, u32),
    content: (u32, u32),
) -> Result<usize, DesktopAdapterError> {
    let bytes = validate_source_dimensions(surface.0, surface.1)?;
    if surface != content {
        return Err(error("TARGET_CHANGED"));
    }
    Ok(bytes)
}

pub(crate) fn validate_visible_pixels(pixels: &[u8]) -> Result<(), DesktopAdapterError> {
    if pixels.is_empty()
        || pixels.len() % 4 != 0
        || pixels.chunks_exact(4).all(|pixel| pixel[3] == 0)
        || pixels.chunks_exact(4).all(|pixel| pixel[..3] == [0, 0, 0])
    {
        return Err(error("TARGET_UNAVAILABLE"));
    }
    Ok(())
}

fn validate_png(bytes: &[u8]) -> Result<(), DesktopAdapterError> {
    crate::bounded_png::validate(bytes, MAX_PNG_BYTES, MAX_IMAGE_EDGE, MAX_IMAGE_PIXELS)
        .map(|_| ())
        .map_err(|_| error("SCAN_LIMIT"))
}

/// Never encode stride padding or bytes outside the validated content surface.
pub(crate) fn copy_rgba_rows(
    raw: &[u8],
    width: u32,
    height: u32,
    row_pitch: usize,
) -> Result<Vec<u8>, DesktopAdapterError> {
    let length = validate_source_dimensions(width, height)?;
    let row_bytes = width as usize * 4;
    let required = row_pitch
        .checked_mul(height as usize)
        .ok_or_else(|| error("SCAN_LIMIT"))?;
    if row_pitch < row_bytes || raw.len() < required {
        return Err(error("ADAPTER_FAILED"));
    }
    let mut pixels = Vec::with_capacity(length);
    for row in raw[..required].chunks_exact(row_pitch) {
        pixels.extend_from_slice(&row[..row_bytes]);
    }
    Ok(pixels)
}

#[cfg(windows)]
pub(crate) fn encode_rgba(
    width: u32,
    height: u32,
    pixels: Vec<u8>,
) -> Result<Vec<u8>, DesktopAdapterError> {
    use std::io::Write;
    if pixels.len() != validate_source_dimensions(width, height)? {
        return Err(error("ADAPTER_FAILED"));
    }
    let source =
        image::RgbaImage::from_raw(width, height, pixels).ok_or_else(|| error("ADAPTER_FAILED"))?;
    let factor = f64::from(MAX_IMAGE_EDGE) / f64::from(width.max(height));
    let image = if factor < 1.0 {
        image::imageops::resize(
            &source,
            (f64::from(width) * factor).floor().max(1.0) as u32,
            (f64::from(height) * factor).floor().max(1.0) as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        source
    };
    struct PngBudget(Vec<u8>);
    impl Write for PngBudget {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) > MAX_PNG_BYTES {
                return Err(io::Error::other("PNG budget"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut output = PngBudget(Vec::new());
    {
        let mut encoder = png::Encoder::new(&mut output, image.width(), image.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|_| error("SCAN_LIMIT"))?;
        writer
            .write_image_data(image.as_raw())
            .map_err(|_| error("SCAN_LIMIT"))?;
        writer.finish().map_err(|_| error("SCAN_LIMIT"))?;
    }
    Ok(output.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_mode_is_exact_and_has_no_targets_on_the_command_line() {
        let args = |values: &[&str]| {
            values
                .iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
                .into_iter()
        };
        assert_eq!(helper_arguments(args(&[])), None);
        assert_eq!(helper_arguments(args(&["--unrelated"])), None);
        assert_eq!(helper_arguments(args(&[HELPER_ARGUMENT])), Some(true));
        assert_eq!(
            helper_arguments(args(&[HELPER_ARGUMENT, "target.exe"])),
            Some(false)
        );
        assert!(parse_target(
            br#"{"process_id":1,"window_handle":1,"executable_path":"x","monitor":1}"#
        )
        .is_err());
        assert!(parse_target(&vec![0; MAX_TARGET_BYTES + 1]).is_err());
    }
    #[test]
    fn frame_size_mismatch_and_blank_pixels_are_not_usable_observations() {
        assert_eq!(validate_frame_size((2, 2), (2, 2)).unwrap(), 16);
        assert!(validate_frame_size((2, 2), (1, 2)).is_err());
        assert!(validate_frame_size((8192, 8192), (8192, 8192)).is_err());
        assert!(validate_visible_pixels(&[0; 16]).is_err());
        assert!(validate_visible_pixels(&[0, 0, 0, 255]).is_err());
        assert!(validate_visible_pixels(&[1, 2, 3, 0]).is_err());
        validate_visible_pixels(&[1, 2, 3, 255]).unwrap();
    }
    #[test]
    fn bounded_pipe_reader_drains_but_never_retains_over_budget_data() {
        let signal = AtomicBool::new(false);
        let data = drain_bounded(io::Cursor::new(vec![1; 100_000]), 16_000, &signal).unwrap();
        assert!(signal.load(Ordering::Acquire));
        assert!(data.len() <= 16_000);
        let signal = AtomicBool::new(false);
        assert_eq!(
            drain_bounded(io::Cursor::new(vec![2; 8192]), 8192, &signal).unwrap(),
            vec![2; 8192]
        );
        assert!(!signal.load(Ordering::Acquire));
    }
    #[test]
    fn row_copy_excludes_padding_and_denies_bad_layouts_before_allocation() {
        assert_eq!(
            copy_rgba_rows(&[1, 2, 3, 4, 99, 99, 5, 6, 7, 8, 99, 99], 1, 2, 6).unwrap(),
            vec![1, 2, 3, 4, 5, 6, 7, 8]
        );
        for (width, height, pitch) in [
            (0, 1, 4),
            (9000, 1, 36_000),
            (8192, 8192, 32_768),
            (2, 1, 4),
            (1, 2, usize::MAX),
        ] {
            assert!(copy_rgba_rows(&[], width, height, pitch).is_err());
        }
    }
    #[cfg(windows)]
    #[test]
    fn synthetic_frame_encodes_and_resizes_without_model_or_desktop_access() {
        let png = encode_rgba(4096, 1, vec![37; 4096 * 4]).unwrap();
        validate_png(&png).unwrap();
        let reader = png::Decoder::new(io::Cursor::new(&png))
            .read_info()
            .unwrap();
        assert_eq!((reader.info().width, reader.info().height), (2048, 1));
        assert!(!format!("{:?}", DesktopWindowImage::from_png_bytes(png)).contains("37, 37"));
    }
    #[cfg(windows)]
    #[test]
    fn process_host_drains_large_pipes_and_reaps_timeouts_and_cancellation() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let powershell = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let make = |script: &str| {
            let mut command = Command::new(&powershell);
            crate::child_process_env::apply_minimal_child_environment(&mut command);
            command
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command
        };
        let output = run_bounded_child(&mut make("$b=New-Object byte[] 1048576; [Console]::OpenStandardOutput().Write($b,0,$b.Length); [Console]::Error.Write('fixture')"), None, MAX_PNG_BYTES, &|| false, Duration::from_secs(5)).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 1_048_576);
        assert_eq!(output.stderr, b"fixture");
        assert_eq!(
            run_bounded_child(
                &mut make("[Console]::Error.Write(('x' * 100000)); Start-Sleep -Seconds 30"),
                None,
                1024,
                &|| false,
                Duration::from_secs(5)
            )
            .err()
            .unwrap()
            .code,
            "SCAN_LIMIT"
        );
        let start = Instant::now();
        assert_eq!(
            run_bounded_child(
                &mut make("Start-Sleep -Seconds 30"),
                None,
                1024,
                &|| false,
                Duration::from_millis(150)
            )
            .err()
            .unwrap()
            .code,
            "SCAN_TIMEOUT"
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        let start = Instant::now();
        assert_eq!(
            run_bounded_child(
                &mut make("Start-Sleep -Seconds 30"),
                None,
                1024,
                &|| start.elapsed() > Duration::from_millis(150),
                Duration::from_secs(5)
            )
            .err()
            .unwrap()
            .code,
            "SCAN_CANCELLED"
        );
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
