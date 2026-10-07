//! File system watcher — monitors the work directory (Documents) for changes.
//!
//! Uses the `notify` crate for cross-platform file system events.
//! Emits "file-change" events to the Tauri frontend when files are created,
//! modified, or deleted.

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use tauri::Emitter;

/// Manages the optional file watcher.
pub struct FileWatcherManager {
    /// Channel to stop the watcher (drop the sender to stop)
    stop_tx: Mutex<Option<Sender<()>>>,
    /// Whether the watcher is currently running
    running: Mutex<bool>,
}

impl Default for FileWatcherManager {
    fn default() -> Self {
        Self::new()
    }
}

impl FileWatcherManager {
    pub fn new() -> Self {
        Self {
            stop_tx: Mutex::new(None),
            running: Mutex::new(false),
        }
    }

    /// Start watching the given directory for file changes.
    /// Events are emitted to the frontend via the Tauri app handle.
    pub fn start_watching(
        &self,
        watch_dir: PathBuf,
        app_handle: tauri::AppHandle,
        db: Option<Arc<Mutex<Connection>>>,
    ) -> Result<(), String> {
        let mut running = self.running.lock().map_err(|e| e.to_string())?;
        if *running {
            return Err("File watcher is already running".to_string());
        }

        if !watch_dir.exists() {
            return Err(format!(
                "Watch directory does not exist: {}",
                watch_dir.display()
            ));
        }

        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

        let (event_tx, event_rx) = std::sync::mpsc::channel::<notify::Result<Event>>();

        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = event_tx.send(res);
            },
            Config::default(),
        )
        .map_err(|e| format!("Failed to create file watcher: {}", e))?;

        watcher
            .watch(&watch_dir, RecursiveMode::Recursive)
            .map_err(|e| format!("Failed to start watching: {}", e))?;

        // Spawn a thread to process events
        let watch_dir_clone = watch_dir.clone();
        std::thread::spawn(move || {
            let _watcher = watcher; // Keep alive
            loop {
                // Check for stop signal
                if stop_rx.try_recv().is_ok() {
                    break;
                }

                // Process file events with a timeout
                match event_rx.recv_timeout(std::time::Duration::from_millis(500)) {
                    Ok(Ok(event)) => {
                        let payload = build_file_event_payload(&event, &watch_dir_clone);
                        if let Some(db) = &db {
                            if let Ok(conn) = db.lock() {
                                let _ = crate::commands::settings::queue_file_change_event(
                                    &conn,
                                    payload.clone(),
                                );
                            }
                        }
                        let _ = app_handle.emit("file-change", payload);
                    }
                    Ok(Err(e)) => {
                        eprintln!("[FileWatcher] Error: {}", e);
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Timeout, loop again to check stop signal
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                }
            }
        });

        *running = true;
        *self.stop_tx.lock().map_err(|e| e.to_string())? = Some(stop_tx);

        eprintln!("[FileWatcher] Started watching: {}", watch_dir.display());
        Ok(())
    }

    /// Stop the file watcher.
    pub fn stop_watching(&self) -> Result<(), String> {
        let mut running = self.running.lock().map_err(|e| e.to_string())?;
        if !*running {
            return Ok(());
        }

        // Drop the sender to signal the watcher thread to stop
        let mut stop_tx = self.stop_tx.lock().map_err(|e| e.to_string())?;
        *stop_tx = None;

        *running = false;
        eprintln!("[FileWatcher] Stopped");
        Ok(())
    }

    /// Check if the watcher is running.
    pub fn is_running(&self) -> bool {
        self.running.lock().map(|r| *r).unwrap_or(false)
    }
}

/// Build a file-change event payload for the frontend.
fn build_file_event_payload(event: &Event, watch_dir: &PathBuf) -> serde_json::Value {
    let kind_str = match event.kind {
        EventKind::Create(_) => "create",
        EventKind::Modify(_) => "modify",
        EventKind::Remove(_) => "remove",
        EventKind::Access(_) => "access",
        _ => "other",
    };

    let paths: Vec<String> = event
        .paths
        .iter()
        .map(|p| {
            // Try to make path relative to watch directory
            p.strip_prefix(watch_dir)
                .unwrap_or(p)
                .to_string_lossy()
                .to_string()
        })
        .collect();

    serde_json::json!({
        "kind": kind_str,
        "paths": paths,
        "timestamp": chrono::Utc::now().timestamp(),
    })
}
