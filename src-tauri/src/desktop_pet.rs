use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

pub(crate) const LABEL: &str = "desktop-pet";
pub(crate) const SUPPORTED: bool = cfg!(windows);
const UNAVAILABLE: &str = "桌宠仅在 Windows 桌面版可用。";

#[derive(Clone, Copy, Serialize)]
pub(crate) struct PetStatus {
    pub enabled: bool,
    pub supported: bool,
}

pub(crate) fn allows_command(window: &str, command: &str) -> bool {
    match command {
        "get_desktop_pet" | "set_desktop_pet" => window == "main",
        "desktop_pet_reset_position" => matches!(window, "main" | LABEL),
        "desktop_pet_start_drag" | "desktop_pet_open_main" | "desktop_pet_hide" => window == LABEL,
        // The pet window does not share the main window's command surface.
        _ => window != LABEL,
    }
}

pub(crate) fn scoped_invoke(
    invoke_app: impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    move |invoke| {
        if !allows_command(
            invoke.message.webview_ref().label(),
            invoke.message.command(),
        ) {
            invoke.resolver.reject("此窗口无法调用该命令。");
            return true;
        }
        invoke_app(invoke)
    }
}

pub(crate) fn show_main(app: &AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("主窗口已关闭。")?;
    window.show().map_err(|error| error.to_string())?;
    if window.is_minimized().map_err(|error| error.to_string())? {
        window.unminimize().map_err(|error| error.to_string())?;
    }
    window.set_focus().map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn get_desktop_pet(app: AppHandle) -> Result<PetStatus, String> {
    #[cfg(windows)]
    return native::status(&app);
    #[cfg(not(windows))]
    {
        let _ = app;
        Ok(PetStatus {
            enabled: false,
            supported: false,
        })
    }
}

#[tauri::command]
pub(crate) async fn set_desktop_pet(app: AppHandle, enabled: bool) -> Result<PetStatus, String> {
    #[cfg(windows)]
    return native::set_enabled(&app, enabled).await;
    #[cfg(not(windows))]
    {
        let _ = (app, enabled);
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
pub(crate) async fn desktop_pet_hide(app: AppHandle) -> Result<(), String> {
    set_desktop_pet(app, false).await.map(|_| ())
}

#[tauri::command]
pub(crate) fn desktop_pet_start_drag(app: AppHandle) -> Result<(), String> {
    app.get_webview_window(LABEL)
        .ok_or(UNAVAILABLE)?
        .start_dragging()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn desktop_pet_open_main(app: AppHandle) -> Result<(), String> {
    show_main(&app)?;
    app.emit_to("main", "desktop-pet-open-chat", ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn desktop_pet_reset_position(app: AppHandle) -> Result<(), String> {
    #[cfg(windows)]
    return native::reset_position(&app);
    #[cfg(not(windows))]
    {
        let _ = app;
        Err(UNAVAILABLE.into())
    }
}

#[cfg(windows)]
pub(crate) use native::{flush, install, on_window_event, toggle_from_tray, DesktopPet};

#[cfg(windows)]
mod native {
    use super::*;
    use serde::Deserialize;
    use std::{fs, io::Write, path::PathBuf, sync::Mutex, time::Duration};
    use tauri::{
        menu::CheckMenuItem, window::Monitor, PhysicalPosition, WebviewUrl, WebviewWindow,
        WebviewWindowBuilder, Window, WindowEvent,
    };

    const WIDTH: f64 = 200.0;
    const HEIGHT: f64 = 220.0;

    #[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
    struct Position {
        x: i32,
        y: i32,
    }

    #[derive(Clone, Copy, Default, Deserialize, Serialize)]
    #[serde(default)]
    struct Preferences {
        enabled: bool,
        position: Option<Position>,
    }

    pub(crate) struct DesktopPet {
        path: PathBuf,
        preferences: Mutex<Preferences>,
        transition: tokio::sync::Mutex<()>,
        save_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    }

    struct TrayToggle(CheckMenuItem<tauri::Wry>);

    impl DesktopPet {
        pub(crate) fn new(root: PathBuf) -> Self {
            let path = root.join("desktop-pet.json");
            let preferences = match fs::read(&path) {
                Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                    eprintln!("[Desktop pet] Invalid preferences: {error}");
                    Preferences::default()
                }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Preferences::default()
                }
                Err(error) => {
                    eprintln!("[Desktop pet] Cannot read preferences: {error}");
                    Preferences::default()
                }
            };
            Self {
                path,
                preferences: Mutex::new(preferences),
                transition: Default::default(),
                save_task: Default::default(),
            }
        }

        fn save(&self, preferences: Preferences) -> Result<(), String> {
            let write = || -> std::io::Result<()> {
                let root = self
                    .path
                    .parent()
                    .expect("pet preferences have a parent directory");
                let mut file = tempfile::NamedTempFile::new_in(root)?;
                serde_json::to_writer(&mut file, &preferences)?;
                file.flush()?;
                file.as_file().sync_all()?;
                file.persist(&self.path).map_err(|error| error.error)?;
                Ok(())
            };
            write().map_err(|error| format!("无法保存桌宠设置：{error}"))
        }
    }

    #[derive(Clone, Copy)]
    struct WorkArea {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        scale: f64,
    }

    impl From<&Monitor> for WorkArea {
        fn from(monitor: &Monitor) -> Self {
            let area = monitor.work_area();
            Self {
                x: area.position.x,
                y: area.position.y,
                width: area.size.width,
                height: area.size.height,
                scale: monitor.scale_factor(),
            }
        }
    }

    fn fit_position(area: WorkArea, position: Option<Position>) -> Position {
        let left = i64::from(area.x);
        let top = i64::from(area.y);
        let right = left + i64::from(area.width);
        let bottom = top + i64::from(area.height);
        let max_x = (right - (WIDTH * area.scale).round() as i64).max(left);
        let max_y = (bottom - (HEIGHT * area.scale).round() as i64).max(top);
        let margin = (16.0 * area.scale).round() as i64;
        let (x, y) = position.map_or((max_x - margin, max_y - margin), |position| {
            (i64::from(position.x), i64::from(position.y))
        });
        Position {
            x: x.clamp(left, max_x) as i32,
            y: y.clamp(top, max_y) as i32,
        }
    }

    fn fitted_position(areas: &[WorkArea], primary: WorkArea, saved: Option<Position>) -> Position {
        let Some(saved) = saved else {
            return fit_position(primary, None);
        };
        areas
            .iter()
            .map(|area| fit_position(*area, Some(saved)))
            .min_by_key(|position| {
                let dx = i128::from(position.x) - i128::from(saved.x);
                let dy = i128::from(position.y) - i128::from(saved.y);
                dx * dx + dy * dy
            })
            .unwrap_or_else(|| fit_position(primary, Some(saved)))
    }

    fn position_window(
        window: &WebviewWindow,
        saved: Option<Position>,
    ) -> Result<Option<Position>, String> {
        let monitors = window
            .available_monitors()
            .map_err(|error| error.to_string())?;
        let primary = window
            .primary_monitor()
            .map_err(|error| error.to_string())?;
        let Some(primary) = primary.as_ref().or_else(|| monitors.first()) else {
            return Ok(None);
        };
        let areas: Vec<_> = monitors.iter().map(WorkArea::from).collect();
        let position = fitted_position(&areas, WorkArea::from(primary), saved);
        window
            .set_position(PhysicalPosition::new(position.x, position.y))
            .map_err(|error| error.to_string())?;
        Ok(Some(position))
    }

    fn show(app: &AppHandle, position: Option<Position>) -> Result<(), String> {
        let window = match app.get_webview_window(LABEL) {
            Some(window) => window,
            None => WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("?desktop-pet".into()))
                .title("AngelBot 桌宠")
                .inner_size(WIDTH, HEIGHT)
                .resizable(false)
                .decorations(false)
                .transparent(true)
                .shadow(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .focused(false)
                .visible(false)
                .build()
                .map_err(|error| error.to_string())?,
        };
        position_window(&window, position)?;
        window.show().map_err(|error| error.to_string())
    }

    pub(super) fn status(app: &AppHandle) -> Result<PetStatus, String> {
        let enabled = app
            .get_webview_window(LABEL)
            .map(|window| window.is_visible())
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or(false);
        Ok(PetStatus {
            enabled,
            supported: true,
        })
    }

    fn publish(app: &AppHandle, status: PetStatus) {
        if let Some(menu) = app.try_state::<TrayToggle>() {
            let _ = menu.0.set_checked(status.enabled);
        }
        let _ = app.emit_to("main", "desktop-pet-changed", status);
    }

    fn hide(app: &AppHandle) -> Result<(), String> {
        if let Some(window) = app.get_webview_window(LABEL) {
            window.hide().map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(super) async fn set_enabled(app: &AppHandle, enabled: bool) -> Result<PetStatus, String> {
        let pet = app.state::<DesktopPet>();
        let _guard = pet.transition.lock().await;
        set_enabled_locked(app, &pet, enabled)
    }

    fn set_enabled_locked(
        app: &AppHandle,
        pet: &DesktopPet,
        enabled: bool,
    ) -> Result<PetStatus, String> {
        let previous = status(app)?;
        let position = pet.preferences.lock().unwrap().position;
        if enabled && !previous.enabled {
            show(app, position)?;
        }
        if !enabled && previous.enabled {
            hide(app)?;
        }
        let result = {
            let mut preferences = pet.preferences.lock().unwrap();
            let next = Preferences {
                enabled,
                ..*preferences
            };
            pet.save(next).map(|()| *preferences = next)
        };
        if let Err(error) = result {
            if enabled != previous.enabled {
                let _ = if previous.enabled {
                    show(app, position)
                } else {
                    hide(app)
                };
            }
            return Err(error);
        }
        let status = PetStatus {
            enabled,
            supported: true,
        };
        publish(app, status);
        Ok(status)
    }

    pub(crate) fn install(app: &AppHandle, item: CheckMenuItem<tauri::Wry>) {
        app.manage(TrayToggle(item));
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let pet = app.state::<DesktopPet>();
            let _guard = pet.transition.lock().await;
            let saved = *pet.preferences.lock().unwrap();
            if saved.enabled {
                match show(&app, saved.position) {
                    Ok(()) => publish(
                        &app,
                        PetStatus {
                            enabled: true,
                            supported: true,
                        },
                    ),
                    Err(error) => eprintln!("[Desktop pet] Cannot restore window: {error}"),
                }
            }
        });
    }

    pub(crate) fn toggle_from_tray(app: &AppHandle) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let pet = app.state::<DesktopPet>();
            let _guard = pet.transition.lock().await;
            let result =
                status(&app).and_then(|status| set_enabled_locked(&app, &pet, !status.enabled));
            if let Err(error) = result {
                if let Ok(current) = status(&app) {
                    publish(&app, current);
                }
                eprintln!("[Desktop pet] Cannot toggle window: {error}");
            }
        });
    }

    fn record_move(app: &AppHandle, position: Position) {
        let pet = app.state::<DesktopPet>();
        {
            let mut preferences = pet.preferences.lock().unwrap();
            if preferences.position == Some(position) {
                return;
            }
            preferences.position = Some(position);
        }
        let mut save_task = pet.save_task.lock().unwrap();
        if let Some(task) = save_task.take() {
            task.abort();
        }
        let app = app.clone();
        *save_task = Some(tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            flush(&app);
        }));
    }

    pub(crate) fn flush(app: &AppHandle) {
        let pet = app.state::<DesktopPet>();
        let preferences = pet.preferences.lock().unwrap();
        if !preferences.enabled && preferences.position.is_none() {
            return;
        }
        if let Err(error) = pet.save(*preferences) {
            eprintln!("[Desktop pet] {error}");
        }
    }

    pub(super) fn reset_position(app: &AppHandle) -> Result<(), String> {
        let window = app.get_webview_window(LABEL).ok_or("桌宠尚未开启。")?;
        let position = position_window(&window, None)?;
        let pet = app.state::<DesktopPet>();
        let mut preferences = pet.preferences.lock().unwrap();
        let next = Preferences {
            position,
            ..*preferences
        };
        pet.save(next)?;
        *preferences = next;
        Ok(())
    }

    pub(crate) fn on_window_event(window: &Window, event: &WindowEvent) {
        let app = window.app_handle();
        match (window.label(), event) {
            (LABEL, WindowEvent::Moved(position)) => record_move(
                app,
                Position {
                    x: position.x,
                    y: position.y,
                },
            ),
            (LABEL, WindowEvent::CloseRequested { api, .. }) => {
                api.prevent_close();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = set_enabled(&app, false).await {
                        eprintln!("[Desktop pet] Cannot close window: {error}");
                    }
                });
            }
            ("main", WindowEvent::CloseRequested { api, .. }) => {
                if status(app).is_ok_and(|status| status.enabled) {
                    api.prevent_close();
                    let _ = window.hide();
                } else if let Some(pet) = app.get_webview_window(LABEL) {
                    // A hidden native window still keeps the runtime alive.
                    // Let closing the main window exit normally when the pet is off.
                    let _ = pet.destroy();
                }
            }
            _ => {}
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn positioning_uses_monitor_work_area_and_dpi() {
            let primary = WorkArea {
                x: 0,
                y: 0,
                width: 1920,
                height: 1040,
                scale: 1.0,
            };
            let second = WorkArea {
                x: -1600,
                y: -100,
                width: 1600,
                height: 1000,
                scale: 1.5,
            };
            assert_eq!(
                fitted_position(&[primary, second], primary, None),
                Position { x: 1704, y: 804 }
            );
            assert_eq!(
                fitted_position(
                    &[primary, second],
                    primary,
                    Some(Position { x: -1500, y: 700 })
                ),
                Position { x: -1500, y: 570 }
            );
            assert_eq!(
                fitted_position(&[primary], primary, Some(Position { x: -1500, y: 700 })),
                Position { x: 0, y: 700 }
            );
        }

        #[test]
        fn preferences_are_off_by_default_and_position_roundtrips() {
            let root = tempfile::tempdir().unwrap();
            let pet = DesktopPet::new(root.path().into());
            assert!(!pet.preferences.lock().unwrap().enabled);
            pet.save(Preferences {
                enabled: true,
                position: Some(Position { x: -500, y: 42 }),
            })
            .unwrap();
            let reopened = DesktopPet::new(root.path().into());
            assert!(reopened.preferences.lock().unwrap().enabled);
            assert_eq!(
                reopened.preferences.lock().unwrap().position,
                Some(Position { x: -500, y: 42 })
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pet_window_uses_only_its_own_commands() {
        assert!(allows_command(LABEL, "desktop_pet_start_drag"));
        assert!(allows_command(LABEL, "desktop_pet_hide"));
        assert!(!allows_command(LABEL, "get_desktop_pet"));
        assert!(!allows_command(LABEL, "send_message"));
        assert!(!allows_command(LABEL, "list_work_dir"));
        assert!(!allows_command("main", "desktop_pet_open_main"));
        assert!(allows_command("main", "desktop_pet_reset_position"));
        assert!(allows_command("main", "get_desktop_pet"));
    }
}
