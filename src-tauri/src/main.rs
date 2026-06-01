// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod parser;

use parser::RawMetadata;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use rayon::prelude::*;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

// Store initial path from CLI args (e.g. "Open With" on macOS)
struct InitialPath(Mutex<Option<String>>);

// ── Session persistence ────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct SessionData {
    folder: String,
    keep: Vec<String>,
    trash: Vec<String>,
    star: Vec<String>,
    current_index: usize,
}

const SESSION_FILE: &str = ".apertinder_session.json";

#[tauri::command]
fn save_session(
    dir_path: String,
    keep: Vec<String>,
    trash: Vec<String>,
    star: Vec<String>,
    current_index: usize,
) -> Result<(), String> {
    let session = SessionData { folder: dir_path.clone(), keep, trash, star, current_index };
    let json = serde_json::to_string(&session)
        .map_err(|e| format!("Failed to serialize session: {}", e))?;
    let session_path = Path::new(&dir_path).join(SESSION_FILE);
    fs::write(session_path, json)
        .map_err(|e| format!("Failed to write session: {}", e))?;
    Ok(())
}

#[tauri::command]
fn load_session(dir_path: String) -> Option<SessionData> {
    let session_path = Path::new(&dir_path).join(SESSION_FILE);
    let json = fs::read_to_string(session_path).ok()?;
    serde_json::from_str(&json).ok()
}

#[tauri::command]
fn clear_session(dir_path: String) -> Result<(), String> {
    let session_path = Path::new(&dir_path).join(SESSION_FILE);
    if session_path.exists() {
        fs::remove_file(session_path)
            .map_err(|e| format!("Failed to clear session: {}", e))?;
    }
    Ok(())
}

// ── File scanning ──────────────────────────────────────────────────────────

#[tauri::command]
fn scan_directory(dir_path: String) -> Result<Vec<RawMetadata>, String> {
    let path = Path::new(&dir_path);
    if !path.exists() || !path.is_dir() {
        return Err("Directory does not exist or is not a directory".to_string());
    }

    let mut file_paths = Vec::new();
    for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let file_path = entry.path();
            // Skip the session file itself and hidden system files
            if file_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with('.'))
                .unwrap_or(false)
            {
                continue;
            }
            if let Some(ext) = file_path.extension().and_then(|e| e.to_str()) {
                let ext_lower = ext.to_lowercase();
                if matches!(
                    ext_lower.as_str(),
                    "arw" | "nef" | "cr2" | "cr3" | "raf" | "dng"
                    | "jpg" | "jpeg" | "png"
                ) {
                    file_paths.push(file_path.to_string_lossy().into_owned());
                }
            }
        }
    }

    let results: Vec<RawMetadata> = file_paths
        .par_iter()
        .filter_map(|path_str| parser::parse_raw_file(path_str).ok())
        .collect();

    Ok(results)
}

// ── Preview fetch ──────────────────────────────────────────────────────────

#[tauri::command]
fn get_raw_preview(path: String, offset: u32, length: u32) -> Result<Vec<u8>, String> {
    let mut file = File::open(&path).map_err(|e| format!("Failed to open file: {}", e))?;
    file.seek(SeekFrom::Start(offset as u64))
        .map_err(|e| format!("Failed to seek to preview: {}", e))?;
    let mut buffer = vec![0u8; length as usize];
    file.read_exact(&mut buffer)
        .map_err(|e| format!("Failed to read preview bytes: {}", e))?;
    Ok(buffer)
}

// ── Culling execution ──────────────────────────────────────────────────────

/// Applies all culling decisions:
/// - keep_list  → moved to `Selected_to_Edit/` in the same folder
/// - star_list  → moved to `Starred/` in the same folder (overrides keep/trash)
/// - trash_list → sent to the OS trash (macOS Trash / Windows Recycle Bin / Linux ~/.Trash)
#[tauri::command]
fn execute_culling_actions(
    keep_list: Vec<String>,
    trash_list: Vec<String>,
    star_list: Vec<String>,
) -> Result<(), String> {
    // Starred files override keep and trash: build a fast lookup set
    let star_set: std::collections::HashSet<&str> =
        star_list.iter().map(|s| s.as_str()).collect();

    // Keep → Selected_to_Edit/
    for file_str in &keep_list {
        if star_set.contains(file_str.as_str()) {
            continue; // star takes precedence
        }
        let file_path = Path::new(file_str);
        if !file_path.exists() {
            continue;
        }
        let parent = file_path.parent().ok_or("Invalid file path")?;
        let target_dir = parent.join("Selected_to_Edit");
        fs::create_dir_all(&target_dir)
            .map_err(|e| format!("Cannot create Selected_to_Edit: {}", e))?;
        let file_name = file_path.file_name().ok_or("Invalid file name")?;
        fs::rename(file_path, target_dir.join(file_name))
            .map_err(|e| format!("Failed to move '{}': {}", file_str, e))?;
    }

    // Star → Starred/
    for file_str in &star_list {
        let file_path = Path::new(file_str);
        if !file_path.exists() {
            continue;
        }
        let parent = file_path.parent().ok_or("Invalid file path")?;
        let target_dir = parent.join("Starred");
        fs::create_dir_all(&target_dir)
            .map_err(|e| format!("Cannot create Starred directory: {}", e))?;
        let file_name = file_path.file_name().ok_or("Invalid file name")?;
        fs::rename(file_path, target_dir.join(file_name))
            .map_err(|e| format!("Failed to move starred '{}': {}", file_str, e))?;
    }

    // Trash → OS trash (recoverable)
    for file_str in &trash_list {
        if star_set.contains(file_str.as_str()) {
            continue; // star takes precedence
        }
        let file_path = Path::new(file_str);
        if !file_path.exists() {
            continue;
        }
        trash::delete(file_path)
            .map_err(|e| format!("Failed to trash '{}': {}", file_str, e))?;
    }

    Ok(())
}

// ── Folder picker ──────────────────────────────────────────────────────────

#[tauri::command]
fn select_folder() -> Option<String> {
    rfd::FileDialog::new()
        .pick_folder()
        .map(|path| path.to_string_lossy().into_owned())
}

/// Returns the folder path passed via CLI args (e.g. macOS "Open With")
#[tauri::command]
fn get_initial_path(state: tauri::State<InitialPath>) -> Option<String> {
    state.0.lock().unwrap().clone()
}

fn main() {
    // Capture CLI argument: first arg that is an existing directory
    let initial_path: Option<String> = std::env::args()
        .skip(1)
        .find(|arg| {
            if arg.starts_with('-') { return false; }
            Path::new(arg).is_dir()
        });

    tauri::Builder::default()
        .manage(InitialPath(Mutex::new(initial_path)))
        .invoke_handler(tauri::generate_handler![
            scan_directory,
            get_raw_preview,
            execute_culling_actions,
            select_folder,
            get_initial_path,
            save_session,
            load_session,
            clear_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
