<div align="center">

<img src="src/logo.png" width="96" alt="Apertin Logo" />

# Apertin

**Ultra-fast, zero-cloud RAW image culler for photographers who shoot thousands.**

[![License: MIT](https://img.shields.io/badge/License-MIT-orange.svg?style=flat-square)](LICENSE)
[![Built with Tauri](https://img.shields.io/badge/Built%20with-Tauri%202-24C8DB?style=flat-square&logo=tauri)](https://tauri.app)
[![Rust](https://img.shields.io/badge/Backend-Rust-CE412B?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![Svelte](https://img.shields.io/badge/Frontend-Svelte-FF3E00?style=flat-square&logo=svelte)](https://svelte.dev/)
[![macOS](https://img.shields.io/badge/macOS-Supported-000000?style=flat-square&logo=apple)](https://www.apple.com/macos/)

> Crafted with ❤️ by [Mustafa Erdem Köşk](https://github.com/mek)

</div>

---

## ✦ What is Apertin?

Apertin is a **local-first, blazing-fast RAW image culling app** built for photographers who come back from a shoot with 400+ RAW files and need to triage them in minutes — not hours.

It works like a card-swipe workflow for your photos. Swipe right to keep, left to trash. Every operation runs entirely on your machine — **no cloud uploads, no subscriptions, no waiting**.

The core engine is written in **Rust** and uses memory-mapped file I/O to extract embedded JPEG previews directly from RAW files at native speed. Your full-resolution RAW files never have to be decoded during culling.

---

## ⚡ Key Features

| Feature | Details |
|---|---|
| 🚀 **Zero-decode preview extraction** | Reads the embedded JPEG directly from RAW binary — no full decode needed |
| 🔥 **Swipe Mode** | Keyboard-driven keep/trash workflow with animated card transitions |
| 👁️ **Browse Mode** | Classic gallery browser with instant prev/next navigation |
| 🔍 **Focus Check Zoom** | Press `Space` to enter pixel-level zoom mode across all images |
| ⭐ **Star Rating** | Mark hero shots with `↑` for selective editing |
| 📊 **EXIF Sidebar** | Camera, lens, shutter, aperture, ISO, focal length at a glance |
| 🗂️ **macOS "Open With"** | Right-click any folder in Finder → Open With → Apertin |
| 🖱️ **Drag & Drop** | Drag a folder onto the app window to start instantly |
| 🗑️ **OS Trash** | Trashed files go to your system recycle bin |
| ✅ **Selected_to_Edit export** | Kept files moved to `Selected_to_Edit/` ready for Lightroom/Capture One |
| 🌑 **Dark mode only** | Premium dark UI — built for low-light post-production environments |
| 🔒 **Fully local** | Zero network requests, zero telemetry, zero cloud |

---

## 📸 Supported Formats

| Format | Camera Brand |
|---|---|
| `.ARW` | Sony (α series) |
| `.CR3` / `.CR2` | Canon (EOS series) |
| `.NEF` | Nikon |
| `.RAF` | Fujifilm |
| `.DNG` | Adobe / Leica / DJI |
| `.JPG` / `.JPEG` | Any camera |
| `.PNG` | Any source |

---

## 🎯 Workflow

```
📁 Open Folder
    │
    ▼
⚡ Rust scans directory & extracts embedded previews (parallel, memory-mapped)
    │
    ▼
🔥 Swipe Mode
    ├── → Keep    (moves to Selected_to_Edit/)
    ├── ← Trash   (sent to OS recycle bin)
    └── ↑ Star    (moves to Starred/)
    │
    ▼
📋 Review Decisions
    ├── Inspect thumbnails
    ├── Restore from trash
    └── Demote keeps to trash
    │
    ▼
✅ Apply — done. Open Selected_to_Edit/ in Lightroom.
```

---

## ⌨️ Keyboard Shortcuts

### Swipe Mode
| Key | Action |
|---|---|
| `→` | Keep image |
| `←` | Trash image |
| `↑` | Toggle star |
| `Space` | Toggle focus zoom (persists across images) |

### Browse Mode
| Key | Action |
|---|---|
| `→` | Next image |
| `←` | Previous image |
| `↑` | Toggle star |
| `Space` | Toggle focus zoom |

---

## 🏗️ Architecture

Apertin is built on a **Rust + Svelte + Tauri** stack. The separation of concerns is clean:

```
┌─────────────────────────────────────────┐
│           Svelte Frontend               │
│  - State machine (welcome/cull/summary) │
│  - Card stack animations                │
│  - Keyboard event bus                   │
│  - Blob URL preview rendering           │
└────────────────┬────────────────────────┘
                 │ Tauri IPC (invoke)
┌────────────────▼────────────────────────┐
│           Rust Backend                  │
│  - scan_directory (parallel WalkDir)    │
│  - parse_raw_file (EXIF + preview)      │
│  - get_raw_preview (mmap byte read)     │
│  - execute_culling_actions (fs moves)   │
│  - select_folder (rfd native dialog)    │
│  - get_initial_path (CLI arg / Open With)│
└─────────────────────────────────────────┘
```

### Why Rust for the core?

- RAW files range from 20MB–100MB each
- Parallel preview extraction via **Rayon** (one thread per file)
- Memory-mapped I/O means preview bytes are read without loading the full file
- Result: 400 Sony ARW files scanned in ~1.2 seconds on M-series Mac

---

## 🛠️ Installation

### Prerequisites

- [Rust](https://rustup.rs/) (stable toolchain)
- [Node.js](https://nodejs.org/) 18+
- [Tauri CLI v2](https://tauri.app/start/prerequisites/)

```bash
# macOS dependencies (if not already installed)
xcode-select --install
```

### Clone & Run

```bash
git clone https://github.com/erdemkosk/apertin.git
cd apertin

npm install
npm run tauri dev
```

### Build for Production

```bash
npm run tauri build
```

The `.dmg` / `.app` bundle will appear in `src-tauri/target/release/bundle/`.

---

## 📂 Project Structure

```
apertin/
├── src/                    # Svelte frontend
│   ├── App.svelte          # Main application component
│   ├── global.css          # Design system (HSL tokens, glassmorphism)
│   ├── logo.png            # App icon
│   └── main.js             # Vite entry point
│
├── src-tauri/              # Rust backend
│   ├── src/
│   │   ├── main.rs         # Tauri commands + CLI arg handler
│   │   └── parser.rs       # RAW EXIF + preview parser
│   ├── Cargo.toml          # Rust dependencies
│   └── tauri.conf.json     # App config + macOS file associations
│
├── index.html              # App shell
├── vite.config.js          # Vite bundler config
└── package.json
```

---

## 🎨 Design Philosophy

Apertin's UI is built around a single principle: **the photo should fill your vision, not the interface**.

- **Dark-first**: Deep volcanic slate backgrounds (#07090e) chosen to match how photographers work in dimmed rooms
- **Glassmorphism panels**: Sidebar and EXIF strip use `backdrop-filter: blur()` so they feel like HUD overlays, not UI chrome
- **Amber accent system**: The `#f97316` amber is the only persistent color — everything else is monochrome or semantic (green = keep, red = trash, gold = star)
- **Plus Jakarta Sans**: Chosen over system fonts for its optical regularity and premium weight range
- **Zero animations for the photo itself** — only chrome elements animate. The photo is always sharp and still.

---

## 🔩 Tech Stack

| Layer | Technology | Why |
|---|---|---|
| Desktop shell | [Tauri 2](https://tauri.app) | Smaller than Electron, native webview, Rust backend |
| Frontend | [Svelte](https://svelte.dev) | Zero-overhead reactivity, no virtual DOM |
| Backend | [Rust](https://www.rust-lang.org/) | Memory safety + parallel performance |
| File walking | [walkdir](https://crates.io/crates/walkdir) | Efficient recursive directory traversal |
| Parallelism | [rayon](https://crates.io/crates/rayon) | Work-stealing thread pool for parallel file parsing |
| Native dialogs | [rfd](https://crates.io/crates/rfd) | Cross-platform Rust file dialog |
| Typography | [Plus Jakarta Sans](https://fonts.google.com/specimen/Plus+Jakarta+Sans) | Premium UI typeface |

---

## 🗺️ Roadmap

- [ ] **Windows support** (HIDPI scaling + file association)
- [ ] **Linux support** (GTK file dialog)  
- [ ] **Collection view** — grid browse with zoom
- [ ] **Color label system** — reject / 1-star / 2-star / pick
- [ ] **Duplicate detection** — flag near-identical shots in burst sequences
- [ ] **XMP sidecar export** — write ratings back as metadata without moving files
- [ ] **Lightroom Classic integration** — open Selected_to_Edit directly in catalog

---

## 🤝 Contributing

PRs are welcome. Open an issue to discuss large changes first.

```bash
# Run in dev mode
npm run tauri dev

# Lint frontend
npx svelte-check

# Check Rust
cd src-tauri && cargo check
```

---

## 📄 License

MIT © Mustafa Erdem Köşk

---

<div align="center">

**If Apertin saved you an hour of culling, give it a ⭐**

*Built for photographers, by a photographer.*

</div>
