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
use tauri::Emitter;

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

const SESSION_FILE: &str = ".apertin_session.json";

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

// ── Similarity grouping ────────────────────────────────────────────────────

/// Hamming distance between two 64-bit dHashes.
fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

#[derive(Debug, Clone, Serialize)]
struct GroupProgress {
    processed: usize,
    total: usize,
}

/// Reads a JPEG from the first 2 MB of a file (fast thumbnail, sufficient for pHash).
fn phash_for_file(file_path: &str) -> Option<u64> {
    let file = File::open(file_path).ok()?;
    let mmap = unsafe { memmap2::MmapOptions::new().map(&file).ok()? };
    // Limit to first 2 MB — embedded thumbnails are always near the start
    let scan_end = mmap.len().min(2 * 1024 * 1024);
    let (offset, length) = parser::scan_for_largest_jpeg(&mmap[..scan_end])?;
    let jpeg = &mmap[offset as usize..(offset + length) as usize];
    parser::compute_phash(jpeg)
}

/// Visual-similarity grouping using **pHash + complete-linkage clustering**.
///
/// ## Why complete-linkage?
/// Single-linkage (chain-linking) suffers from the "bridge" problem:
///   A ≈ B, B ≈ C  →  A and C end up in the same group even if dist(A,C) > threshold.
/// Complete-linkage fixes this: a photo joins a group only when it is within
/// `threshold` of **every** existing member. Groups therefore stay tight and
/// semantically consistent.
///
/// ## Why pHash (DCT-based) instead of dHash?
/// dHash compares adjacent pixel pairs at 9×8 — it is extremely sensitive to
/// small exposure / focus changes common in burst shooting.
/// pHash extracts 8×8 low-frequency DCT coefficients from a 32×32 thumbnail.
/// Low frequencies encode global scene structure; noise, blur, and minor
/// exposure shifts are high-frequency artifacts and are automatically ignored.
///
/// ## Threshold guide (Hamming distance over 64 bits):
///   ≤  6 → strict  (near-identical burst shots)
///   ≤ 10 → normal  (same scene, varying exposure/framing)
///   ≤ 15 → loose   (similar subject, different angle)
///
/// Runs in spawn_blocking so the async runtime and UI remain responsive.
/// Progress events carry only the counter (not the full assignments array).
#[tauri::command]
async fn analyze_groups(
    app: tauri::AppHandle,
    file_paths: Vec<String>,
    threshold: u32,
) -> Result<Vec<Option<usize>>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let threshold = threshold.min(64);
        let total = file_paths.len();

        // Phase 1: compute all pHashes in parallel (rayon)
        let hashes: Vec<Option<u64>> = file_paths
            .par_iter()
            .map(|p| phash_for_file(p))
            .collect();

        // Phase 2: complete-linkage online clustering (sequential, order-stable)
        let mut assignments: Vec<Option<usize>> = vec![None; total];
        // group_hashes[g] = all pHash values of members already in group g
        let mut group_hashes: Vec<Vec<u64>> = Vec::new();

        // Emit at most ~20 progress events
        let step = (total / 20).max(5);

        for (i, hash_opt) in hashes.iter().enumerate() {
            match hash_opt {
                None => {
                    // No extractable thumbnail → own singleton group
                    assignments[i] = Some(group_hashes.len());
                    group_hashes.push(vec![]);
                }
                Some(hash) => {
                    let hash = *hash;
                    // Complete-linkage: only join a group if this hash is within
                    // `threshold` of EVERY member already in that group.
                    let best = group_hashes
                        .iter()
                        .enumerate()
                        .find(|(_, members)| {
                            !members.is_empty()
                                && members.iter().all(|&mh| hamming(hash, mh) <= threshold)
                        })
                        .map(|(gid, _)| gid);

                    match best {
                        Some(gid) => {
                            assignments[i] = Some(gid);
                            group_hashes[gid].push(hash);
                        }
                        None => {
                            assignments[i] = Some(group_hashes.len());
                            group_hashes.push(vec![hash]);
                        }
                    }
                }
            }

            if (i + 1) % step == 0 || i + 1 == total {
                app.emit("group-progress", GroupProgress { processed: i + 1, total }).ok();
            }
        }

        Ok(assignments)
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Opens a URL in the system's default browser (cross-platform).
#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    open::that(url).map_err(|e| e.to_string())
}

// ── File scanning ──────────────────────────────────────────────────────────

#[tauri::command]
fn scan_directory(dir_path: String) -> Result<Vec<RawMetadata>, String> {
    let path = Path::new(&dir_path);
    if !path.exists() || !path.is_dir() {
        return Err("Directory does not exist or is not a directory".to_string());
    }

    // These output directories must never be re-scanned to prevent nested folder creation
    const SKIP_DIRS: &[&str] = &["Selected_to_Edit", "Starred", ".trash"];

    let mut file_paths = Vec::new();
    for entry in WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| {
            // For directories: skip output dirs by exact name match
            if e.file_type().is_dir() {
                let name = e.file_name().to_str().unwrap_or("");
                return !SKIP_DIRS.contains(&name);
            }
            // For files: skip hidden/dot files (session file, .DS_Store, etc.)
            let name = e.file_name().to_str().unwrap_or("");
            !name.starts_with('.')
        })
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            let file_path = entry.path();
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
            analyze_groups,
            open_external_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
