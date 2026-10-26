//! One desktop process per durable data directory, before recovery or dispatch.
//! The open Windows handle is the authority; a leftover filename is not a lock.

use std::{fs::File, io, path::Path};

pub(crate) fn acquire(root: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::create_dir_all(root)?;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(root.join("instance.lock"))
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows runtime required",
        ))
    }
}

pub(crate) fn notify_failure(error: &io::Error) {
    let (title, message) = if error.raw_os_error() == Some(32) {
        (
            "AngelBot · 已在运行",
            "此资料目录已有 AngelBot 在运行，请使用已打开的窗口。未启动第二个实例。",
        )
    } else {
        (
            "AngelBot · 无法启动",
            "无法锁定本地资料目录，未启动或恢复任务。请检查目录是否可写后重试。",
        )
    };
    eprintln!("[Startup] instance guard unavailable: {error}");
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
        let title: Vec<_> = title.encode_utf16().chain(Some(0)).collect();
        let message: Vec<_> = message.encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }
    #[cfg(not(windows))]
    eprintln!("{title}: {message}");
}

pub(crate) fn notify_storage_failure(error: &io::Error) {
    if error.raw_os_error() == Some(32) {
        return notify_failure(error);
    }
    eprintln!("[Storage] profile preparation failed: {error}");
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
        let title: Vec<_> = "AngelBot · 资料迁移未完成"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let reason = crate::profile_storage::explanation(error)
            .unwrap_or("请关闭其他 AngelBot 窗口，并检查磁盘空间和目录权限。");
        let message: Vec<_> = format!("{reason}\n\n没有丢弃新旧资料。请勿手动删除新旧目录；检查并备份后，可使用 --migrate-storage 明确重试。").encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }
}

pub(crate) fn notify_storage_rollback() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
        let title: Vec<_> = "AngelBot · 资料迁移已回滚"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let message: Vec<_> = "旧资料已恢复，尚未启动应用。再次正常打开时将使用恢复目录；检查并备份后，可用 --migrate-storage 明确重试。".encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn exclusive_handle_blocks_same_root_and_releases_without_deleting_the_file() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let first = acquire(root.path()).unwrap();
        assert_eq!(acquire(root.path()).unwrap_err().raw_os_error(), Some(32));
        let independent = acquire(other.path()).unwrap();
        drop(first);
        assert!(root.path().join("instance.lock").is_file());
        let reopened = acquire(root.path()).unwrap();
        drop(reopened);
        drop(independent);
    }
}
