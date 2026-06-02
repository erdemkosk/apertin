<script>
  import { onMount } from 'svelte';
  import logo from './logo.png';
  
  // Attempt to import tauri core invoke. Fallback if running in browser dev.
  let invoke;
  try {
    import('@tauri-apps/api/core').then(mod => {
      invoke = mod.invoke;
    });
  } catch (e) {
    console.warn("Tauri API not available (browser mode)", e);
  }

  // App version — injected from tauri.conf.json via Vite, overridden by
  // native Tauri getVersion() at runtime (same source, just a double-check).
  let appVersion = import.meta.env.VITE_APP_VERSION ?? '0.0.0';

  try {
    import('@tauri-apps/api/app').then(mod => {
      mod.getVersion().then(v => { appVersion = v; }).catch(() => {});
    });
  } catch (_) {}

  // ── Update checker ────────────────────────────────────────────────────────
  const GITHUB_REPO = 'erdemkosk/apertin';
  let updateAvailable = false;
  let latestVersion = '';
  let updateCheckDone = false;
  let checkingUpdate = false;
  let updateReleaseUrl = '';

  async function checkForUpdates(silent = true) {
    if (checkingUpdate) return;
    checkingUpdate = true;
    try {
      const res = await fetch(
        `https://api.github.com/repos/${GITHUB_REPO}/releases/latest`,
        { headers: { Accept: 'application/vnd.github+json' } }
      );
      if (!res.ok) throw new Error('network');
      const data = await res.json();
      // tag_name is like "v0.1.0-build.12" — extract semver prefix
      const match = data.tag_name?.match(/^v?(\d+\.\d+\.\d+)/);
      if (match) {
        latestVersion = match[1];
        updateReleaseUrl = data.html_url ?? '';
        const current = appVersion.replace(/[^\d.]/g, '');
        updateAvailable = latestVersion !== current && latestVersion > current;
      }
    } catch (_) {
      if (!silent) alert('Güncelleme kontrolü başarısız. İnternet bağlantınızı kontrol edin.');
    } finally {
      checkingUpdate = false;
      updateCheckDone = true;
    }
  }

  // App States
  let state = 'welcome'; // 'welcome' | 'culling' | 'summary' | 'complete'
  let mode = 'gallery'; // 'gallery' | 'swipe' (active culling)
  let dirPath = '';
  let files = [];
  let currentIndex = 0;
  
  // Selection Lists
  let keepList = new Set();
  let trashList = new Set();
  let starList = new Set();

  // Loading States
  let loading = false;
  let loadingMessage = '';

  // Active Image Preview URL
  let currentPreviewUrl = '';
  let previewLoading = false;

  // Swiping / Card animation states
  let swipeState = ''; // 'keep' | 'trash' | 'star' | ''
  let animating = false;

  // Zoom / Focus check states
  let isZoomed = false;
  let mouseX = 50;
  let mouseY = 50;

  // Real-time keyboard press states for indicator lighting
  let activeKeys = {
    ArrowRight: false,
    ArrowLeft: false,
    ArrowUp: false,
    Space: false
  };

  // Review tab filter state ('trash' | 'keep' | 'star')
  let reviewFilter = 'trash';
  // Map of Path -> Blob URL for summary reviews
  let summaryPreviews = {};

  // ── Similarity grouping ───────────────────────────────────────────────────

  let groupAssignments = [];   // number|null per file index
  let groupAnalyzing = false;
  let groupProgress = { processed: 0, total: 0 };
  let groupUnlistener = null;
  // 'time' | 'visual'
  let groupMode = 'time';
  // Time gap presets (seconds)
  const TIME_GAPS = [{ label: '30s', value: 30 }, { label: '2 min', value: 120 }, { label: '5 min', value: 300 }];
  let timeGap = 120;
  // Visual similarity threshold presets (Hamming, 0-64)
  // pHash thresholds — Hamming distance over 64 bits.
  // pHash uses DCT low-frequencies → more stable than dHash; lower values work.
  //   6 = burst / near-identical only
  //  10 = same scene, varying exposure/framing   (recommended default)
  //  15 = similar subject, different angle
  const THRESHOLDS = [{ label: 'Burst', value: 6 }, { label: 'Normal', value: 10 }, { label: 'Loose', value: 15 }];
  let visualThreshold = 10;

  $: groupSizes = (() => {
    const s = {};
    for (const gid of groupAssignments) {
      if (gid != null) s[gid] = (s[gid] || 0) + 1;
    }
    return s;
  })();

  // Re-order indices so all members of a group are rendered consecutively,
  // anchored at the position of the earliest member in each group.
  $: displayOrder = (() => {
    const placed = new Set();
    const order = [];
    for (let i = 0; i < files.length; i++) {
      if (placed.has(i)) continue;
      const gid = groupAssignments[i] ?? null;
      if (gid != null && (groupSizes[gid] ?? 1) > 1) {
        // Insert all members of this group together
        for (let j = i; j < files.length; j++) {
          if (!placed.has(j) && groupAssignments[j] === gid) {
            order.push(j);
            placed.add(j);
          }
        }
      } else {
        order.push(i);
        placed.add(i);
      }
    }
    return order;
  })();

  const GROUP_COLORS = [
    '#f59e0b','#3b82f6','#10b981','#8b5cf6','#ef4444',
    '#06b6d4','#f97316','#84cc16','#ec4899','#14b8a6',
    '#6366f1','#a78bfa',
  ];
  function groupColor(gid) {
    return gid == null ? 'transparent' : GROUP_COLORS[gid % GROUP_COLORS.length];
  }

  // Parse EXIF date "2026:06:02 12:30:15" → ms timestamp (or null)
  function parseExifDate(str) {
    if (!str) return null;
    const iso = str.replace(/^(\d{4}):(\d{2}):(\d{2})/, '$1-$2-$3');
    const t = Date.parse(iso);
    return isNaN(t) ? null : t;
  }

  // Instant time-based grouping (pure JS, no Rust call)
  function groupByTime(gapSec) {
    const gapMs = gapSec * 1000;
    const assignments = new Array(files.length).fill(null);
    const indexed = files.map((f, i) => ({ i, t: parseExifDate(f.date_time) }))
                         .filter(x => x.t != null)
                         .sort((a, b) => a.t - b.t);

    let gid = 0, lastT = null;
    for (const { i, t } of indexed) {
      if (lastT == null || t - lastT > gapMs) { gid++; }
      assignments[i] = gid;
      lastT = t;
    }
    return assignments;
  }

  async function runGrouping() {
    if (files.length === 0 || groupAnalyzing) return;

    if (groupMode === 'time') {
      groupAssignments = groupByTime(timeGap);
      return;
    }

    // Visual mode — call Rust, progressive events
    groupAnalyzing = true;
    groupProgress = { processed: 0, total: files.length };
    groupAssignments = new Array(files.length).fill(null);

    try {
      const { listen } = await import('@tauri-apps/api/event');
      if (groupUnlistener) groupUnlistener();
      // Only update the progress counter — never touch groupAssignments here
      // to avoid re-rendering the entire file list on every event.
      groupUnlistener = await listen('group-progress', (ev) => {
        groupProgress = { processed: ev.payload.processed, total: ev.payload.total };
      });
    } catch (_) {}

    try {
      if (invoke) {
        const final = await invoke('analyze_groups', {
          filePaths: files.map(f => f.file_path),
          threshold: visualThreshold,
        });
        // Single assignment update — one re-render at the very end
        groupAssignments = final;
      }
    } catch (e) {
      console.error('Group analysis failed:', e);
    } finally {
      groupAnalyzing = false;
      if (groupUnlistener) { groupUnlistener(); groupUnlistener = null; }
    }
  }

  // ── Session persistence ──────────────────────────────────────────────────

  async function saveSession() {
    if (!invoke || !dirPath || files.length === 0) return;
    try {
      await invoke('save_session', {
        dirPath,
        keep: Array.from(keepList),
        trash: Array.from(trashList),
        star: Array.from(starList),
        currentIndex,
      });
    } catch (_) { /* non-critical */ }
  }

  async function clearSession() {
    if (!invoke || !dirPath) return;
    try { await invoke('clear_session', { dirPath }); } catch (_) {}
  }

  // Jump to specific index
  function jumpToImage(index) {
    if (index >= 0 && index < files.length) {
      currentIndex = index;
      isZoomed = false;
    }
  }

  let nextPreviewUrl = '';

  // Reactive preview preloading
  let loadingPreviewsForIndex = -1;
  $: if (files.length > 0 && currentIndex < files.length && currentIndex !== loadingPreviewsForIndex) {
    loadingPreviewsForIndex = currentIndex;
    preloadPreviews(currentIndex);
  }

  // Clean up main object URL to prevent RAM leaks
  function cleanupPreviewUrl() {
    if (currentPreviewUrl) {
      if (currentPreviewUrl.startsWith('blob:')) {
        URL.revokeObjectURL(currentPreviewUrl);
      }
      currentPreviewUrl = '';
    }
  }

  function cleanupNextPreviewUrl() {
    if (nextPreviewUrl) {
      if (nextPreviewUrl.startsWith('blob:')) {
        URL.revokeObjectURL(nextPreviewUrl);
      }
      nextPreviewUrl = '';
    }
  }

  // Fetch preview URL helper
  async function fetchPreviewUrl(index) {
    if (index < 0 || index >= files.length) return '';
    const file = files[index];
    try {
      if (invoke) {
        const bytes = await invoke('get_raw_preview', {
          path: file.file_path,
          offset: file.preview_offset,
          length: file.preview_length
        });
        const ext = file.file_path.split('.').pop()?.toLowerCase() ?? '';
        const mimeType = ext === 'png' ? 'image/png' : 'image/jpeg';
        const blob = new Blob([new Uint8Array(bytes)], { type: mimeType });
        return URL.createObjectURL(blob);
      } else {
        // Mock preview for browser development
        return 'https://picsum.photos/1600/1000?random=' + index;
      }
    } catch (e) {
      console.error("Failed to load preview:", e);
      return '';
    }
  }

  // Preload current and next preview to enable zero-latency transitions
  async function preloadPreviews(index) {
    previewLoading = true;
    try {
      // Load current preview
      const currentUrl = await fetchPreviewUrl(index);
      cleanupPreviewUrl();
      currentPreviewUrl = currentUrl;

      // Preload next preview in background
      if (index + 1 < files.length) {
        const nextUrl = await fetchPreviewUrl(index + 1);
        cleanupNextPreviewUrl();
        nextPreviewUrl = nextUrl;
      } else {
        cleanupNextPreviewUrl();
        nextPreviewUrl = '';
      }
    } finally {
      previewLoading = false;
    }
  }

  let dragActive = false;

  // Native Select Folder dialog
  async function browseFolder() {
    try {
      if (invoke) {
        const selected = await invoke('select_folder');
        if (selected) {
          dirPath = selected;
          startScan();
        }
      } else {
        // Mock browse for development/browser
        dirPath = '/Users/username/Pictures/RAW_Mock';
        startScan();
      }
    } catch (e) {
      console.error("Failed to select folder:", e);
      alert("Error selecting folder: " + e);
    }
  }

  function handleDragEnter(e) {
    e.preventDefault();
    dragActive = true;
  }

  // Allow drop events by preventing default browser action
  function handleDragOver(e) {
    e.preventDefault();
    dragActive = true;
  }

  function handleDragLeave(e) {
    e.preventDefault();
    dragActive = false;
  }

  function handleDrop(e) {
    e.preventDefault();
    dragActive = false;
    
    if (e.dataTransfer.files && e.dataTransfer.files.length > 0) {
      const file = e.dataTransfer.files[0];
      // In Tauri webview, native file drops attach the full absolute OS path in the 'path' property
      const path = file.path;
      if (path) {
        dirPath = path;
        startScan();
      } else {
        alert("Drop operation not supported: path unavailable. Use Browse Folder button.");
      }
    }
  }

  // Scan Directory command
  async function startScan() {
    if (!dirPath.trim()) return;
    loading = true;
    loadingMessage = 'Scanning directory and extracting metadata...';
    
    try {
      if (invoke) {
        files = await invoke('scan_directory', { dirPath: dirPath.trim() });
      } else {
        // Mock data for browser development
        await new Promise(resolve => setTimeout(resolve, 1500));
        files = Array.from({ length: 15 }, (_, i) => ({
          file_path: `${dirPath}/photo_${i + 1}.ARW`,
          file_name: `photo_${i + 1}.ARW`,
          camera_make: 'Sony',
          camera_model: 'ILCE-7RM3',
          lens_model: 'FE 24-70mm F2.8 GM',
          width: 7952,
          height: 5304,
          iso: 100 * (i + 1),
          aperture: 2.8,
          shutter_speed: '1/250',
          focal_length: 50,
          date_time: '2026:06:02 12:30:15',
          orientation: 1,
          preview_offset: 0,
          preview_length: 1024 * 1024
        }));
      }

      if (files.length === 0) {
        alert("No supported image files (.ARW, .NEF, .CR2, .CR3, .RAF, .DNG, .JPG, .JPEG, .PNG) found in this directory.");
        loading = false;
        return;
      }

      currentIndex = 0;
      keepList = new Set();
      trashList = new Set();
      starList = new Set();

      // Restore previous session for this folder if one exists
      if (invoke) {
        try {
          const session = await invoke('load_session', { dirPath: dirPath.trim() });
          if (session && session.folder === dirPath.trim()) {
            keepList = new Set(session.keep);
            trashList = new Set(session.trash);
            starList = new Set(session.star);
            currentIndex = Math.min(session.current_index, files.length - 1);
          }
        } catch (_) { /* no session — start fresh */ }
      }

      mode = 'gallery';
      state = 'culling';
    } catch (e) {
      alert("Error scanning directory: " + e);
      console.error(e);
    } finally {
      loading = false;
    }
  }

  // Culling Operations (Swipe Mode)
  function keepImage() {
    if (animating) return;
    swipeState = 'keep';
    animating = true;
    const current = files[currentIndex];
    
    setTimeout(() => {
      keepList.add(current.file_path);
      trashList.delete(current.file_path);
      
      keepList = new Set(keepList);
      trashList = new Set(trashList);
      
      nextImage();
      swipeState = '';
      animating = false;
      saveSession();
    }, 450);
  }

  function trashImage() {
    if (animating) return;
    swipeState = 'trash';
    animating = true;
    const current = files[currentIndex];

    setTimeout(() => {
      trashList.add(current.file_path);
      keepList.delete(current.file_path);

      trashList = new Set(trashList);
      keepList = new Set(keepList);

      nextImage();
      swipeState = '';
      animating = false;
      saveSession();
    }, 450);
  }

  function toggleStar() {
    const current = files[currentIndex];
    if (starList.has(current.file_path)) {
      starList.delete(current.file_path);
    } else {
      starList.add(current.file_path);
    }
    starList = new Set(starList);
    saveSession();

    // Flash star animation badge
    swipeState = 'star';
    setTimeout(() => {
      if (swipeState === 'star') swipeState = '';
    }, 350);
  }

  // Navigation helpers
  function nextImage() {
    if (currentIndex < files.length - 1) {
      currentIndex += 1;
      mouseX = 50;
      mouseY = 50;
      saveSession();
    } else {
      isZoomed = false;
      enterSummaryState();
    }
  }

  function prevImage() {
    if (currentIndex > 0) {
      currentIndex -= 1;
      mouseX = 50;
      mouseY = 50;
      saveSession();
    }
  }

  // Handle mode switches
  function toggleMode() {
    mode = mode === 'gallery' ? 'swipe' : 'gallery';
    isZoomed = false;
  }

  // Mouse zoom tracking
  function handleMouseMove(e) {
    if (!isZoomed) return;
    const rect = e.currentTarget.getBoundingClientRect();
    mouseX = ((e.clientX - rect.left) / rect.width) * 100;
    mouseY = ((e.clientY - rect.top) / rect.height) * 100;
  }

  function toggleZoom() {
    isZoomed = !isZoomed;
  }

  // Keyboard Event Handlers
  function handleKeyDown(e) {
    // Highlight physical key press
    let keyName = e.key;
    if (e.code === 'Space' || e.key === ' ') {
      keyName = 'Space';
    }
    if (keyName in activeKeys) {
      activeKeys[keyName] = true;
      activeKeys = { ...activeKeys };
    }

    if (state !== 'culling') return;
    
    // Prevent default scrolling keys
    if (['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Space', ' '].includes(e.key)) {
      e.preventDefault();
    }

    if (mode === 'swipe') {
      if (e.key === 'ArrowRight') {
        keepImage();
      } else if (e.key === 'ArrowLeft') {
        trashImage();
      } else if (e.key === 'ArrowUp') {
        toggleStar();
      }
    } else {
      // Gallery Mode navigation
      if (e.key === 'ArrowRight') {
        nextImage();
      } else if (e.key === 'ArrowLeft') {
        prevImage();
      } else if (e.key === 'ArrowUp') {
        toggleStar();
      }
    }

    // Zoom Focus key - Toggle on keypress instead of hold
    if (e.key === ' ' || e.code === 'Space') {
      isZoomed = !isZoomed;
    }
  }

  // Reset zoom key states on keyup
  function handleKeyUp(e) {
    let keyName = e.key;
    if (e.code === 'Space' || e.key === ' ') {
      keyName = 'Space';
    }
    if (keyName in activeKeys) {
      activeKeys[keyName] = false;
      activeKeys = { ...activeKeys };
    }
  }

  // Load previews dynamically for whichever review category is active
  async function loadSummaryPreviews() {
    loadingMessage = 'Loading review thumbnails...';
    loading = true;

    try {
      cleanupSummaryPreviews();
      const listToLoad = reviewFilter === 'trash'
        ? Array.from(trashList)
        : reviewFilter === 'star'
          ? Array.from(starList)
          : Array.from(keepList);
      for (const path of listToLoad) {
        const file = files.find(f => f.file_path === path);
        if (!file) continue;

        if (invoke) {
          const bytes = await invoke('get_raw_preview', {
            path: file.file_path,
            offset: file.preview_offset,
            length: file.preview_length
          });
          const ext = file.file_path.split('.').pop()?.toLowerCase() ?? '';
          const mime = ext === 'png' ? 'image/png' : 'image/jpeg';
          const blob = new Blob([new Uint8Array(bytes)], { type: mime });
          summaryPreviews[path] = URL.createObjectURL(blob);
        } else {
          summaryPreviews[path] = 'https://picsum.photos/400/250?random=' + Math.random();
        }
      }
      summaryPreviews = { ...summaryPreviews };
    } catch (e) {
      console.error("Failed to load review thumbnails:", e);
    } finally {
      loading = false;
    }
  }

  // Load reviews on transition to Summary State
  async function enterSummaryState() {
    state = 'summary';
    isZoomed = false;
    reviewFilter = 'trash';
    await loadSummaryPreviews();
  }

  // Change review filter and load thumbnails
  async function changeReviewFilter(filter) {
    if (reviewFilter === filter) return;
    reviewFilter = filter;
    await loadSummaryPreviews();
  }

  // Restore file from Trash -> back to Keep
  async function restoreImage(path) {
    trashList.delete(path);
    keepList.add(path);

    trashList = new Set(trashList);
    keepList = new Set(keepList);

    if (summaryPreviews[path]) {
      if (summaryPreviews[path].startsWith('blob:')) {
        URL.revokeObjectURL(summaryPreviews[path]);
      }
      delete summaryPreviews[path];
      summaryPreviews = { ...summaryPreviews };
    }

    saveSession();
    await loadSummaryPreviews();
  }

  // Unstar a file (from summary starred tab)
  async function unstarImage(path) {
    starList.delete(path);
    starList = new Set(starList);

    if (summaryPreviews[path]) {
      if (summaryPreviews[path].startsWith('blob:')) {
        URL.revokeObjectURL(summaryPreviews[path]);
      }
      delete summaryPreviews[path];
      summaryPreviews = { ...summaryPreviews };
    }

    saveSession();
    await loadSummaryPreviews();
  }

  // Move file from Keep -> to Trash
  async function demoteImageToTrash(path) {
    keepList.delete(path);
    trashList.add(path);

    keepList = new Set(keepList);
    trashList = new Set(trashList);

    if (summaryPreviews[path]) {
      if (summaryPreviews[path].startsWith('blob:')) {
        URL.revokeObjectURL(summaryPreviews[path]);
      }
      delete summaryPreviews[path];
      summaryPreviews = { ...summaryPreviews };
    }

    saveSession();
    await loadSummaryPreviews();
  }

  // Revoke all cached thumbnail URLs
  function cleanupSummaryPreviews() {
    for (const url of Object.values(summaryPreviews)) {
      if (url && url.startsWith('blob:')) {
        URL.revokeObjectURL(url);
      }
    }
    summaryPreviews = {};
  }

  // Execute Culling Moves/Deletes
  async function confirmCulling() {
    loading = true;
    loadingMessage = 'Applying culling actions…';

    // Starred files override keep/trash — they always go to Starred/
    const starArr = Array.from(starList);
    const starSet = new Set(starArr);
    const keepArr = Array.from(keepList).filter(p => !starSet.has(p));
    const trashArr = Array.from(trashList).filter(p => !starSet.has(p));

    try {
      if (invoke) {
        await invoke('execute_culling_actions', {
          keepList: keepArr,
          trashList: trashArr,
          starList: starArr,
        });
      } else {
        await new Promise(resolve => setTimeout(resolve, 2000));
        console.log("Mock culling applied: Keep", keepArr, "Trash", trashArr, "Star", starArr);
      }
      cleanupSummaryPreviews();
      await clearSession();
      state = 'complete';
    } catch (e) {
      alert("Error applying culling actions: " + e);
      console.error(e);
    } finally {
      loading = false;
    }
  }

  // Reset Session
  function startNewSession() {
    cleanupPreviewUrl();
    cleanupNextPreviewUrl();
    cleanupSummaryPreviews();
    files = [];
    currentIndex = 0;
    keepList = new Set();
    trashList = new Set();
    starList = new Set();
    groupAssignments = [];
    groupAnalyzing = false;
    groupProgress = { processed: 0, total: 0 };
    if (groupUnlistener) { groupUnlistener(); groupUnlistener = null; }
    state = 'welcome';
  }

  onMount(async () => {
    // Ensure window has keyboard focus for Tauri
    window.focus();
    document.addEventListener('click', () => window.focus(), { passive: true });

    // Check for updates silently after a short delay
    setTimeout(() => checkForUpdates(true), 4000);

    // Check if app was launched with a folder argument (e.g. macOS "Open With")
    if (invoke) {
      try {
        const initialPath = await invoke('get_initial_path');
        if (initialPath) {
          dirPath = initialPath;
          await startScan();
        }
      } catch (e) {
        // Not critical — just start normally
        console.warn('No initial path from CLI:', e);
      }
    }

    return () => {
      cleanupPreviewUrl();
      cleanupNextPreviewUrl();
      cleanupSummaryPreviews();
    };
  });
</script>

<svelte:window on:keydown={handleKeyDown} on:keyup={handleKeyUp} />

<!-- Main Wrapper -->
<div class="app-container {isZoomed ? 'fullscreen-active' : ''}">
  
  <!-- Loading Layer -->
  {#if loading}
    <div class="loading-overlay">
      <div class="spinner"></div>
      <p class="loading-text">{loadingMessage}</p>
    </div>
  {/if}

  <!-- Sidebar Gallery Panel -->
  {#if state === 'culling' || state === 'summary'}
    <aside class="sidebar glass-panel">
      <div class="brand">
        <span class="brand-gradient">
          <span style="color: hsl(var(--text-primary))">Aper</span><span style="color: hsl(var(--accent-amber))">tin</span>
          <span class="brand-version">v{appVersion}</span>
          {#if updateAvailable}
            <span class="update-badge" title="Yeni sürüm: v{latestVersion}">● Güncelleme var</span>
          {/if}
        </span>
        <div class="brand-sub">
          Developed with ❤️ by Mustafa Erdem Köşk
          {#if updateAvailable}
            <a class="update-link" href={updateReleaseUrl} target="_blank" rel="noreferrer">
              v{latestVersion} indir →
            </a>
          {:else}
            <button
              class="update-check-btn"
              on:click={() => checkForUpdates(false)}
              disabled={checkingUpdate}
              title="Güncelleme kontrol et"
            >
              {checkingUpdate ? '⏳' : updateCheckDone ? '✓ Güncel' : '↻ Güncelle?'}
            </button>
          {/if}
        </div>
      </div>

      <!-- Culling Progress bar -->
      <div class="progress-section">
        <div class="progress-header">
          <span>DECK PROGRESS</span>
          <span class="badge">{currentIndex + 1} / {files.length}</span>
        </div>
        <div class="progress-bar-container">
          <div class="progress-bar" style="width: {((currentIndex + 1) / files.length) * 100}%"></div>
        </div>
      </div>

      <!-- Live statistics indicators -->
      <div class="stats-grid">
        <div class="stat-card keep">
          <span class="stat-num">{keepList.size}</span>
          <span class="stat-label">Kept</span>
        </div>
        <div class="stat-card trash">
          <span class="stat-num">{trashList.size}</span>
          <span class="stat-label">Trash</span>
        </div>
        <div class="stat-card star">
          <span class="stat-num">{starList.size}</span>
          <span class="stat-label">Starred</span>
        </div>
      </div>

      <!-- Grouping panel -->
      <div class="group-panel">
        <div class="group-mode-row">
          <button class="gmode-btn {groupMode === 'time' ? 'active' : ''}"
                  on:click={() => { groupMode = 'time'; }}>⏱ Time</button>
          <button class="gmode-btn {groupMode === 'visual' ? 'active' : ''}"
                  on:click={() => { groupMode = 'visual'; }}>⬡ Visual</button>
        </div>
        {#if groupMode === 'time'}
          <div class="group-opts-row">
            {#each TIME_GAPS as g}
              <button class="opt-chip {timeGap === g.value ? 'active' : ''}"
                      on:click={() => timeGap = g.value}>{g.label}</button>
            {/each}
          </div>
        {:else}
          <div class="group-opts-row">
            {#each THRESHOLDS as t}
              <button class="opt-chip {visualThreshold === t.value ? 'active' : ''}"
                      on:click={() => visualThreshold = t.value}>{t.label}</button>
            {/each}
          </div>
        {/if}
        <button class="group-run-btn {groupAnalyzing ? 'analyzing' : ''}"
                on:click={runGrouping}
                disabled={groupAnalyzing || files.length === 0}>
          {#if groupAnalyzing}
            <span class="group-spinner"></span>
            {groupProgress.processed}/{groupProgress.total}
          {:else if groupAssignments.length > 0}
            ⟳ Re-group
          {:else}
            Group Photos
          {/if}
        </button>
        {#if groupAssignments.length > 0 && !groupAnalyzing}
          <span class="group-result-label">
            {Object.values(groupSizes).filter(s => s > 1).length} groups · {Object.values(groupSizes).filter(s => s > 1).reduce((a, b) => a + b, 0)} photos
          </span>
        {/if}
      </div>

      <!-- Gallery Index Slider -->
      <div class="file-list-title">FOLDER DIRECTORY</div>
      <div class="file-list">
        {#each displayOrder as idx, pos}
          {@const file = files[idx]}
          {@const gid = groupAssignments[idx] ?? null}
          {@const inGroup = gid != null && (groupSizes[gid] ?? 1) > 1}
          {@const isGroupFirst = inGroup && (pos === 0 || groupAssignments[displayOrder[pos - 1]] !== gid)}

          {#if isGroupFirst}
            <div class="group-header" style="--gc: {groupColor(gid)}">
              <span class="group-dot"></span>
              Group · {groupSizes[gid]} photos
            </div>
          {/if}

          <button
            class="file-item {currentIndex === idx ? 'active' : ''} {inGroup ? 'in-group' : ''}"
            style={inGroup ? `--gc: ${groupColor(gid)}` : ''}
            on:click={() => jumpToImage(idx)}
          >
            <div class="file-info-left">
              <span class="file-idx">{idx + 1}</span>
              <span class="file-name">{file.file_name}</span>
            </div>
            <div class="file-badges">
              {#if keepList.has(file.file_path)}
                <span class="item-badge keep">✓</span>
              {/if}
              {#if trashList.has(file.file_path)}
                <span class="item-badge trash">✗</span>
              {/if}
              {#if starList.has(file.file_path)}
                <span class="item-badge star">★</span>
              {/if}
            </div>
          </button>
        {/each}
      </div>

      <!-- Sidebar summary reviews -->
      <div class="sidebar-footer">
        <button 
          class="glow-btn finish-btn" 
          on:click={enterSummaryState}
        >
          Review Decisions
        </button>
      </div>
    </aside>
  {/if}

  <!-- Main Viewport -->
  <main class="main-content">
    
    <!-- Top Action Header -->
    <header class="top-bar glass-panel">
      {#if state === 'welcome'}
        <div class="top-title">Welcome to Apertin</div>
      {:else if state === 'culling'}
        <div class="top-title breadcrumb">
          <span class="folder-path">{dirPath}</span>
          <span class="chevron">/</span>
          <span class="active-file">{files[currentIndex]?.file_name || ''}</span>
        </div>
        
        <!-- Toggle button between Modes -->
        <div class="mode-toggles">
          <button 
            class="mode-btn {mode === 'gallery' ? 'active' : ''}" 
            on:click={() => mode = 'gallery'}
          >
            👁️ Browse Mode
          </button>
          <button 
            class="mode-btn {mode === 'swipe' ? 'active' : ''}" 
            on:click={() => mode = 'swipe'}
          >
            🔥 Swipe Mode
          </button>
          <div class="vertical-divider"></div>
          <button class="new-session-btn" on:click={startNewSession}>Reset Folder</button>
        </div>
      {:else if state === 'summary'}
        <div class="top-title">Culling Decisions Summary</div>
        <button class="new-session-btn" on:click={() => state = 'culling'}>Back to Grid</button>
      {:else if state === 'complete'}
        <div class="top-title">Summary Action Completed</div>
      {/if}
    </header>

    <!-- Content Area -->
    <div class="content-body">
      
      <!-- WELCOME STATE -->
      {#if state === 'welcome'}
        <div class="welcome-container glass-panel">
          <div class="welcome-logo-container">
            <img src={logo} alt="Apertin Logo" class="welcome-logo-img" />
          </div>
          <h1 class="welcome-heading">
            <span class="wordmark-apt">Aper</span><span class="wordmark-tin">tin</span>
          </h1>
          <p class="welcome-tagline">RAW Image Culler <span class="welcome-version">v{appVersion}</span></p>
          <p class="welcome-desc">
            Ultra-fast, zero-cloud RAW image culler. Mapped and compiled on your local machine to triage thousands of photos in seconds.
          </p>
          <p class="welcome-byline">Crafted with ❤️ by Mustafa Erdem Köşk</p>
          
          <!-- Drag and drop zone with clickable browse trigger -->
          <div 
            class="drag-zone {dragActive ? 'drag-active' : ''}"
            on:dragenter={handleDragEnter}
            on:dragover={handleDragOver}
            on:dragleave={handleDragLeave}
            on:drop={handleDrop}
            on:click={browseFolder}
          >
            <div class="drag-icon-container">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" class="drag-svg">
                <path stroke-linecap="round" stroke-linejoin="round" d="M12 16.5V9.75m0 0l3 3m-3-3l-3 3M6.75 19.5a4.5 4.5 0 01-1.41-8.775 5.25 5.25 0 0110.233-2.33 3 3 0 013.758 3.848A3.752 3.752 0 0118 19.5H6.75z" />
              </svg>
            </div>
            
            <div class="drag-text-group">
              <span class="drag-title">Drag & Drop your RAW photos folder here</span>
              <span class="drag-or">or</span>
              <span class="glow-btn browse-btn">
                <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="btn-svg">
                  <path stroke-linecap="round" stroke-linejoin="round" d="M2.25 12.75V12A2.25 2.25 0 014.5 9.75h15a2.25 2.25 0 012.25 2.25v.75m-8.69-6.44l-2.12-2.12a1.5 1.5 0 00-1.061-.44H4.5A2.25 2.25 0 002.25 6v12a2.25 2.25 0 002.25 2.25h15A2.25 2.25 0 0021.75 18V9a2.25 2.25 0 00-2.25-2.25h-5.379a1.5 1.5 0 01-1.06-.44z" />
                </svg>
                Browse Directory
              </span>
            </div>
            
            {#if dirPath}
              <div class="selected-path-indicator" on:click|stopPropagation>
                <span class="indicator-label">Selected:</span>
                <span class="indicator-path">{dirPath}</span>
                <button class="small-scan-btn" on:click={startScan}>Scan</button>
              </div>
            {/if}
          </div>
          
          <div class="shortcuts-legend">
            <h3 class="legend-title">KEYBOARD SHORTCUTS</h3>
            <div class="legend-grid">
              <div class="legend-item">
                <span class="legend-mode">TINDER MODE:</span>
                <div>
                  <kbd class="kbd-hint {activeKeys.ArrowRight ? 'active-press keep-press' : ''}">→</kbd> Keep
                  <kbd class="kbd-hint {activeKeys.ArrowLeft ? 'active-press trash-press' : ''}">←</kbd> Trash
                </div>
              </div>
              <div class="legend-item">
                <span class="legend-mode">BROWSE MODE:</span>
                <div>
                  <kbd class="kbd-hint {activeKeys.ArrowRight ? 'active-press keep-press' : ''}">→</kbd> Next
                  <kbd class="kbd-hint {activeKeys.ArrowLeft ? 'active-press trash-press' : ''}">←</kbd> Prev
                </div>
              </div>
              <div class="legend-item">
                <span class="legend-mode">GENERAL:</span>
                <div>
                  <kbd class="kbd-hint {activeKeys.ArrowUp ? 'active-press star-press' : ''}">↑</kbd> Star / Starred
                </div>
              </div>
              <div class="legend-item">
                <span class="legend-mode">FOCUS ZOOM:</span>
                <div>
                  <kbd class="kbd-hint {activeKeys.Space ? 'active-press' : ''}">Space</kbd> Sharpness Zoom (Hold)
                </div>
              </div>
            </div>
          </div>
        </div>

      <!-- CULLING STATE -->
      {:else if state === 'culling'}
        <div class="culler-container">
          
          <!-- Swipe Mode Deck -->
          <div class="deck-area">
            
            <div class="card-stack {mode}">
              {#if mode === 'swipe'}
                <!-- Physical photo stack background layers -->
                {#if files.length - currentIndex > 2}
                  <div class="card-underlay under-2"></div>
                {/if}
                {#if files.length - currentIndex > 1}
                  <div class="card-underlay under-1"></div>
                {/if}
              {/if}

              <div 
                class="image-card glass-panel {swipeState} {mode}"
                style="animation: {swipeState === 'keep' ? 'swipe-right-out 0.45s forwards' : swipeState === 'trash' ? 'swipe-left-out 0.45s forwards' : swipeState === 'star' ? 'swipe-up-out 0.35s forwards' : 'none'}"
              >
                <!-- Indicator overlays (Swipe Mode only) -->
                {#if mode === 'swipe'}
                  <div class="swipe-overlay keep {swipeState === 'keep' ? 'show' : ''}">KEEP</div>
                  <div class="swipe-overlay trash {swipeState === 'trash' ? 'show' : ''}">TRASH</div>
                  <div class="swipe-overlay star {swipeState === 'star' ? 'show' : ''}">STARRED</div>
                {/if}

                <!-- Image view container -->
                <div 
                  class="image-viewport"
                  on:mousemove={handleMouseMove}
                  on:dblclick={toggleZoom}
                  style="cursor: {isZoomed ? 'zoom-out' : 'zoom-in'}"
                >
                  {#if previewLoading}
                    <div class="viewport-loader">
                      <div class="spinner"></div>
                    </div>
                  {/if}

                  {#if currentPreviewUrl}
                    <img 
                      src={currentPreviewUrl} 
                      alt="RAW Preview" 
                      class="preview-img {isZoomed ? 'zoomed' : ''}"
                      style="transform-origin: {mouseX}% {mouseY}%;"
                    />
                  {:else}
                    <div class="no-preview">No Preview Available</div>
                  {/if}

                  <!-- Subtle zoom exit hint -->
                  {#if isZoomed}
                    <div class="zoom-hint">SPACE — exit zoom</div>
                  {/if}

                </div>
              </div>
            </div>

            <!-- Manual Navigation Overlays (Active during Browse Mode) -->
            {#if mode === 'gallery'}
              <button class="nav-arrow left" on:click={prevImage} disabled={currentIndex === 0}>‹</button>
              <button class="nav-arrow right" on:click={nextImage} disabled={currentIndex === files.length - 1}>›</button>
            {/if}

            <!-- Floating mechanical keyboard visualizer dock -->
            <div class="keyboard-helper-dock glass-panel">
              <div class="dock-key-item">
                <kbd class="kbd-hint {activeKeys.ArrowLeft ? 'active-press trash-press' : ''}">←</kbd>
                <span class="dock-key-label">{mode === 'swipe' ? 'Trash' : 'Prev'}</span>
              </div>
              <div class="dock-key-item">
                <kbd class="kbd-hint {activeKeys.ArrowUp ? 'active-press star-press' : ''}">↑</kbd>
                <span class="dock-key-label">Star</span>
              </div>
              <div class="dock-key-item">
                <kbd class="kbd-hint {activeKeys.ArrowRight ? 'active-press keep-press' : ''}">→</kbd>
                <span class="dock-key-label">{mode === 'swipe' ? 'Keep' : 'Next'}</span>
              </div>
              <div class="dock-key-item">
                <kbd class="kbd-hint {activeKeys.Space ? 'active-press' : ''}">Space</kbd>
                <span class="dock-key-label">Zoom</span>
              </div>
            </div>

          </div>

          <!-- Bottom EXIF Metadata readouts -->
          <footer class="exif-panel glass-panel">
            <div class="exif-item">
              <span class="exif-label">CAMERA</span>
              <span class="exif-val">{files[currentIndex]?.camera_make || 'N/A'} {files[currentIndex]?.camera_model || ''}</span>
            </div>
            <div class="exif-divider"></div>
            <div class="exif-item">
              <span class="exif-label">LENS</span>
              <span class="exif-val">{files[currentIndex]?.lens_model || 'Unknown Lens'}</span>
            </div>
            <div class="exif-divider"></div>
            <div class="exif-item">
              <span class="exif-label">EXPOSURE</span>
              <span class="exif-val">
                {files[currentIndex]?.shutter_speed || 'N/A'}s @ ƒ/{files[currentIndex]?.aperture || 'N/A'}
              </span>
            </div>
            <div class="exif-divider"></div>
            <div class="exif-item">
              <span class="exif-label">ISO</span>
              <span class="exif-val">ISO {files[currentIndex]?.iso || 'N/A'}</span>
            </div>
            <div class="exif-divider"></div>
            <div class="exif-item">
              <span class="exif-label">FOCAL LENGTH</span>
              <span class="exif-val">{files[currentIndex]?.focal_length ? files[currentIndex].focal_length + 'mm' : 'N/A'}</span>
            </div>
          </footer>
        </div>

      <!-- SUMMARY / REVIEWS STATE -->
      {:else if state === 'summary'}
        <div class="summary-dashboard">
          
          <div class="summary-header">
            <h2 class="summary-heading">Review Culling Decisions</h2>
            <p class="summary-desc">Confirm files that will be preserved versus moved to the trash bucket before writing changes to disk.</p>
          </div>

          <div class="summary-layout">
            
            <!-- Statistics row / tab switcher -->
            <div class="summary-stats-box glass-panel">
              <button 
                class="summary-metric stat-tab {reviewFilter === 'keep' ? 'active-tab keep' : ''}"
                on:click={() => changeReviewFilter('keep')}
              >
                <span class="metric-num keep">{keepList.size}</span>
                <span class="metric-label">To Keep → /Selected_to_Edit</span>
              </button>
              <button 
                class="summary-metric stat-tab {reviewFilter === 'trash' ? 'active-tab trash' : ''}"
                on:click={() => changeReviewFilter('trash')}
              >
                <span class="metric-num trash">{trashList.size}</span>
                <span class="metric-label">To Trash → OS Recycle Bin</span>
              </button>
              <button 
                class="summary-metric stat-tab {reviewFilter === 'star' ? 'active-tab star' : ''}"
                on:click={() => changeReviewFilter('star')}
              >
                <span class="metric-num star">{starList.size}</span>
                <span class="metric-label">Starred → /Starred</span>
              </button>
            </div>

            <!-- Scrollable Grid of Filtered Items -->
            <div class="trash-gallery-section glass-panel">
              <h3 class="trash-gallery-title">
                {#if reviewFilter === 'trash'}
                  TO TRASH — will be sent to OS recycle bin ({trashList.size})
                {:else if reviewFilter === 'keep'}
                  TO KEEP — will be moved to /Selected_to_Edit ({keepList.size})
                {:else}
                  STARRED — will be moved to /Starred ({starList.size})
                {/if}
              </h3>
              
              {#if reviewFilter === 'trash'}
                {#if trashList.size === 0}
                  <div class="empty-trash-message">
                    No images marked for trash. All images will be kept!
                  </div>
                {:else}
                  <div class="trash-grid">
                    {#each Array.from(trashList) as filePath}
                      <div class="trash-card">
                        <img 
                          src={summaryPreviews[filePath] || 'https://picsum.photos/400/250'} 
                          alt="Trashed Preview" 
                          class="trash-thumb"
                        />
                        <div class="trash-overlay">
                          <button class="restore-btn" on:click={() => restoreImage(filePath)}>
                            Keep Instead
                          </button>
                        </div>
                        <div class="trash-card-info">
                          <span class="trash-filename">{filePath.split('/').pop()}</span>
                        </div>
                      </div>
                    {/each}
                  </div>
                {/if}

              {:else if reviewFilter === 'keep'}
                {#if keepList.size === 0}
                  <div class="empty-trash-message">
                    No images marked to keep. All images will be trashed!
                  </div>
                {:else}
                  <div class="trash-grid">
                    {#each Array.from(keepList) as filePath}
                      <div class="trash-card keep-border">
                        <img 
                          src={summaryPreviews[filePath] || 'https://picsum.photos/400/250'} 
                          alt="Kept Preview" 
                          class="trash-thumb"
                        />
                        <div class="trash-overlay">
                          <button class="demote-btn" on:click={() => demoteImageToTrash(filePath)}>
                            Move to Trash
                          </button>
                        </div>
                        <div class="trash-card-info">
                          <span class="trash-filename">{filePath.split('/').pop()}</span>
                        </div>
                      </div>
                    {/each}
                  </div>
                {/if}

              {:else}
                <!-- Star tab -->
                {#if starList.size === 0}
                  <div class="empty-trash-message">
                    No images starred. Use ↑ while culling to mark favorites.
                  </div>
                {:else}
                  <div class="trash-grid">
                    {#each Array.from(starList) as filePath}
                      <div class="trash-card star-border">
                        <img 
                          src={summaryPreviews[filePath] || 'https://picsum.photos/400/250'} 
                          alt="Starred Preview" 
                          class="trash-thumb"
                        />
                        <div class="trash-overlay">
                          <button class="unstar-btn" on:click={() => unstarImage(filePath)}>
                            Remove Star
                          </button>
                        </div>
                        <div class="trash-card-info">
                          <span class="trash-filename">★ {filePath.split('/').pop()}</span>
                        </div>
                      </div>
                    {/each}
                  </div>
                {/if}
              {/if}
            </div>

          </div>

          <!-- Bottom Action Confirmation -->
          <div class="summary-actions-bar">
            <button class="back-btn" on:click={() => state = 'culling'}>Go Back & Review</button>
            <button class="glow-btn confirm-btn" on:click={confirmCulling}>
              Apply — {keepList.size} keep · {starList.size} star · {trashList.size} trash
            </button>
          </div>
        </div>

      <!-- COMPLETE / SUCCESS STATE -->
      {:else if state === 'complete'}
        <div class="complete-container glass-panel">
          <div class="success-icon">✓</div>
          <h1 class="complete-heading">All Actions Applied!</h1>
          <p class="complete-desc">
            Your culling session is complete.<br/>
            Kept photos → <code>/Selected_to_Edit</code><br/>
            Starred photos → <code>/Starred</code><br/>
            Trashed photos → OS Recycle Bin (recoverable)
          </p>
          <div class="complete-buttons">
            <button class="glow-btn" on:click={startNewSession}>Cull Another Folder</button>
          </div>
        </div>
      {/if}

    </div>

  </main>
</div>

<style>
  /* Layout modifications */
  .mode-toggles {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .mode-btn {
    background: transparent;
    border: 1px solid hsl(var(--border-muted));
    color: hsl(var(--text-secondary));
    padding: 6px 14px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 600;
    transition: all 0.15s ease;
  }

  .mode-btn:hover {
    background: hsl(var(--bg-input));
    color: hsl(var(--text-primary));
  }

  .mode-btn.active {
    background: hsl(var(--accent-amber) / 0.15);
    color: hsl(var(--accent-amber));
    border-color: hsl(var(--accent-amber) / 0.5);
  }

  .vertical-divider {
    width: 1px;
    height: 20px;
    background: hsl(var(--border-muted));
    margin: 0 4px;
  }

  /* ── Similarity grouping ─────────────────────────────────────────────── */
  .group-panel {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 6px 0 10px;
    border-bottom: 1px solid hsl(var(--border-muted));
    margin-bottom: 6px;
  }

  .group-mode-row {
    display: flex;
    gap: 4px;
  }

  .gmode-btn {
    flex: 1;
    background: hsl(var(--bg-input) / 0.5);
    border: 1px solid hsl(var(--border-muted));
    color: hsl(var(--text-secondary));
    font-size: 10px;
    font-weight: 700;
    padding: 4px 6px;
    border-radius: 5px;
    cursor: pointer;
    transition: all 0.12s ease;
    letter-spacing: 0.03em;
  }

  .gmode-btn.active {
    background: hsl(var(--accent-amber) / 0.15);
    color: hsl(var(--accent-amber));
    border-color: hsl(var(--accent-amber) / 0.5);
  }

  .gmode-btn:hover:not(.active) {
    background: hsl(var(--bg-input));
    color: hsl(var(--text-primary));
  }

  .group-opts-row {
    display: flex;
    gap: 4px;
  }

  .opt-chip {
    flex: 1;
    background: transparent;
    border: 1px solid hsl(var(--border-muted));
    color: hsl(var(--text-muted));
    font-size: 10px;
    font-weight: 600;
    padding: 3px 4px;
    border-radius: 4px;
    cursor: pointer;
    transition: all 0.12s ease;
    text-align: center;
  }

  .opt-chip.active {
    border-color: hsl(var(--accent-amber) / 0.6);
    color: hsl(var(--accent-amber));
    background: hsl(var(--accent-amber) / 0.08);
  }

  .opt-chip:hover:not(.active) {
    color: hsl(var(--text-secondary));
    border-color: hsl(var(--border-muted) / 0.8);
  }

  .group-run-btn {
    width: 100%;
    background: hsl(var(--bg-input) / 0.6);
    border: 1px solid hsl(var(--border-muted));
    color: hsl(var(--text-secondary));
    font-size: 11px;
    font-weight: 700;
    padding: 6px 10px;
    border-radius: 6px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 5px;
    transition: all 0.15s ease;
  }

  .group-run-btn:hover:not(:disabled) {
    background: hsl(var(--bg-input));
    color: hsl(var(--text-primary));
    border-color: hsl(var(--accent-amber) / 0.4);
  }

  .group-run-btn.analyzing {
    color: hsl(var(--accent-amber));
    border-color: hsl(var(--accent-amber) / 0.4);
  }

  .group-run-btn:disabled { opacity: 0.4; cursor: not-allowed; }

  .group-spinner {
    display: inline-block;
    width: 8px;
    height: 8px;
    border: 1.5px solid hsl(var(--accent-amber) / 0.3);
    border-top-color: hsl(var(--accent-amber));
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
  }

  @keyframes spin { to { transform: rotate(360deg); } }

  .group-result-label {
    font-size: 10px;
    color: hsl(var(--text-muted));
    text-align: center;
  }

  .group-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px 2px;
    font-size: 10px;
    font-weight: 700;
    color: var(--gc, hsl(var(--text-muted)));
    text-transform: uppercase;
    letter-spacing: 0.06em;
    margin-top: 4px;
  }

  .group-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--gc, hsl(var(--text-muted)));
    flex-shrink: 0;
  }

  .file-item.in-group {
    border-left: 2px solid var(--gc, transparent);
    padding-left: 6px;
  }

  /* ── /Similarity grouping ─────────────────────────────────────────────── */

  .brand-version {
    font-size: 10px;
    font-weight: 500;
    color: hsl(var(--text-muted));
    margin-left: 6px;
    vertical-align: middle;
    letter-spacing: 0.05em;
  }

  .update-badge {
    font-size: 9px;
    font-weight: 600;
    color: hsl(var(--accent-amber));
    margin-left: 6px;
    vertical-align: middle;
    animation: pulse-badge 2s ease-in-out infinite;
  }

  @keyframes pulse-badge {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.5; }
  }

  .update-link {
    display: inline-block;
    margin-left: 6px;
    font-size: 10px;
    font-weight: 600;
    color: hsl(var(--accent-amber));
    text-decoration: none;
    padding: 1px 6px;
    border: 1px solid hsl(var(--accent-amber) / 0.5);
    border-radius: 4px;
  }
  .update-link:hover { background: hsl(var(--accent-amber) / 0.15); }

  .update-check-btn {
    background: none;
    border: none;
    cursor: pointer;
    font-size: 10px;
    color: hsl(var(--text-muted));
    padding: 0;
    margin-left: 6px;
    transition: color 0.2s;
  }
  .update-check-btn:hover:not(:disabled) { color: hsl(var(--accent-amber)); }
  .update-check-btn:disabled { opacity: 0.5; cursor: default; }

  .welcome-version {
    font-size: 13px;
    font-weight: 500;
    color: hsl(var(--text-muted));
    margin-left: 6px;
  }

  /* Keyboard shortcut legends */
  .legend-mode {
    font-weight: 700;
    color: hsl(var(--text-primary));
    font-size: 11px;
    margin-right: 4px;
  }

  /* Manual navigation arrows */
  .nav-arrow {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    width: 44px;
    height: 44px;
    border-radius: 50%;
    background: hsl(var(--bg-card) / 0.8);
    border: 1px solid hsl(var(--border-muted));
    color: hsl(var(--text-primary));
    font-size: 24px;
    font-weight: 300;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    z-index: 50;
    transition: all 0.15s ease;
    backdrop-filter: blur(4px);
  }

  .nav-arrow:hover:not(:disabled) {
    background: hsl(var(--bg-input));
    border-color: hsl(var(--accent-amber) / 0.5);
  }

  .nav-arrow:disabled {
    opacity: 0.2;
    cursor: not-allowed;
  }

  .nav-arrow.left { left: 24px; }
  .nav-arrow.right { right: 24px; }

  /* Summary Dashboard layouts */
  .summary-dashboard {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .summary-header {
    text-align: left;
    margin-bottom: 20px;
  }

  .summary-heading {
    font-family: var(--font-display);
    font-size: 26px;
    font-weight: 800;
    letter-spacing: -0.02em;
  }

  .summary-desc {
    color: hsl(var(--text-secondary));
    font-size: 13px;
    margin-top: 4px;
  }

  .summary-layout {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 20px;
    overflow: hidden;
  }

  .summary-stats-box {
    display: flex;
    align-items: center;
    justify-content: space-around;
    padding: 16px 24px;
    border-radius: 10px;
  }

  .summary-metric {
    display: flex;
    flex-direction: column;
    align-items: center;
  }

  .metric-num {
    font-family: var(--font-display);
    font-size: 32px;
    font-weight: 800;
    line-height: 1.1;
  }

  .metric-num.keep { color: hsl(var(--accent-keep)); }
  .metric-num.trash { color: hsl(var(--accent-trash)); }
  .metric-num.star { color: hsl(var(--accent-star)); }

  .metric-label {
    font-size: 11px;
    font-weight: 600;
    color: hsl(var(--text-secondary));
    margin-top: 4px;
    letter-spacing: 0.05em;
  }

  .trash-gallery-section {
    flex: 1;
    border-radius: 12px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    padding: 20px;
  }

  .trash-gallery-title {
    font-family: var(--font-display);
    font-size: 11px;
    font-weight: 800;
    letter-spacing: 0.1em;
    color: hsl(var(--text-muted));
    margin-bottom: 16px;
  }

  .empty-trash-message {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: hsl(var(--text-secondary));
    font-size: 14px;
    border: 2px dashed hsl(var(--border-muted));
    border-radius: 8px;
    background: hsl(var(--bg-darker) / 0.3);
  }

  .trash-grid {
    flex: 1;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 16px;
    overflow-y: auto;
    padding-right: 4px;
  }

  .trash-card {
    position: relative;
    border-radius: 8px;
    overflow: hidden;
    background: hsl(var(--bg-darker));
    border: 1px solid hsl(var(--border-muted));
    aspect-ratio: 3 / 2;
    display: flex;
    flex-direction: column;
  }

  .trash-thumb {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .trash-overlay {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: rgba(0, 0, 0, 0.7);
    backdrop-filter: blur(4px);
    display: flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    transition: opacity 0.2s ease;
  }

  .trash-card:hover .trash-overlay {
    opacity: 1;
  }

  .restore-btn {
    background: hsl(var(--accent-keep));
    color: #fff;
    font-weight: 700;
    font-size: 11px;
    padding: 6px 14px;
    border-radius: 4px;
    box-shadow: 0 4px 10px hsl(var(--accent-keep-glow));
    transition: all 0.15s ease;
  }

  .restore-btn:hover {
    transform: scale(1.05);
    background: #10b981;
  }

  .demote-btn {
    background: hsl(var(--accent-trash));
    color: #fff;
    font-weight: 700;
    font-size: 11px;
    padding: 6px 14px;
    border-radius: 4px;
    box-shadow: 0 4px 10px rgba(239, 68, 68, 0.3);
    transition: all 0.15s ease;
  }

  .demote-btn:hover {
    transform: scale(1.05);
    background: #dc2626;
  }

  .unstar-btn {
    background: hsl(var(--accent-star));
    color: #000;
    font-weight: 700;
    font-size: 11px;
    padding: 6px 14px;
    border-radius: 4px;
    box-shadow: 0 4px 10px rgba(245, 158, 11, 0.3);
    transition: all 0.15s ease;
  }

  .unstar-btn:hover {
    transform: scale(1.05);
    filter: brightness(1.1);
  }

  .keep-border {
    border: 2px solid hsl(var(--accent-keep) / 0.5);
  }

  .star-border {
    border: 2px solid hsl(var(--accent-star) / 0.5);
  }

  /* Active summary tab highlights */
  .stat-tab {
    cursor: pointer;
    transition: all 0.15s ease;
    border: 1px solid transparent;
    border-radius: 8px;
  }

  .stat-tab:hover {
    background: hsl(var(--bg-input) / 0.5);
  }

  .active-tab.keep {
    border-color: hsl(var(--accent-keep) / 0.5);
    background: hsl(var(--accent-keep) / 0.08);
  }

  .active-tab.trash {
    border-color: hsl(var(--accent-trash) / 0.5);
    background: hsl(var(--accent-trash) / 0.08);
  }

  .active-tab.star {
    border-color: hsl(var(--accent-star) / 0.5);
    background: hsl(var(--accent-star) / 0.08);
  }

  .trash-card-info {
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    background: linear-gradient(to top, rgba(0,0,0,0.8), transparent);
    padding: 12px 8px 6px;
  }

  .trash-filename {
    display: block;
    font-size: 10px;
    color: hsl(var(--text-primary));
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-weight: 600;
  }

  .summary-actions-bar {
    display: flex;
    justify-content: flex-end;
    gap: 16px;
    padding-top: 20px;
    border-top: 1px solid hsl(var(--border-muted));
  }

  .confirm-btn {
    background: linear-gradient(135deg, hsl(var(--accent-trash)), #dc2626);
    box-shadow: 0 4px 14px 0 rgba(239, 68, 68, 0.25);
  }

  .confirm-btn:hover {
    box-shadow: 0 6px 20px 0 rgba(239, 68, 68, 0.45);
  }

  .back-btn {
    font-family: var(--font-body);
    font-size: 13px;
    font-weight: 600;
    color: hsl(var(--text-secondary));
    background: hsl(var(--bg-input) / 0.5);
    border: 1px solid hsl(var(--border-muted));
    border-radius: 8px;
    padding: 10px 22px;
    transition: all 0.15s ease;
  }

  .back-btn:hover {
    background: hsl(var(--bg-input));
    color: hsl(var(--text-primary));
  }

  /* Welcoming styles copy-paste/revisions for safety */
  .culler-container {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    overflow: hidden;
  }

  .deck-area {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    position: relative;
    overflow: hidden;
  }

  .image-card {
    position: relative;
    width: 100%;
    height: 95%;
    max-width: 900px;
    border-radius: 12px;
    overflow: hidden;
    box-shadow: 0 20px 50px rgba(0,0,0,0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    transition: transform 0.22s ease-out, border-color 0.15s ease-out, box-shadow 0.15s ease-out;
  }

  .image-card.keep {
    border-color: hsl(var(--accent-keep));
    box-shadow: 0 0 40px var(--accent-keep-glow);
  }

  .image-card.trash {
    border-color: hsl(var(--accent-trash));
    box-shadow: 0 0 40px var(--accent-trash-glow);
  }

  .image-card.star {
    border-color: hsl(var(--accent-star));
    box-shadow: 0 0 40px var(--accent-star-glow);
  }

  .image-viewport {
    position: relative;
    width: 100%;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    background: #000;
  }

  .preview-img {
    width: 100%;
    height: 100%;
    object-fit: contain;
    transition: transform 0.1s ease-out;
  }

  .preview-img.zoomed {
    object-fit: none;
    width: auto;
    height: auto;
    max-width: none;
    max-height: none;
  }

  .viewport-loader {
    position: absolute;
    z-index: 20;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .no-preview {
    color: hsl(var(--text-muted));
    font-size: 16px;
    font-weight: 500;
  }

  .zoom-hint {
    position: absolute;
    bottom: 16px;
    right: 16px;
    z-index: 30;
    background: rgba(0, 0, 0, 0.55);
    color: rgba(255, 255, 255, 0.45);
    padding: 5px 10px;
    border-radius: 6px;
    font-family: var(--font-display);
    font-weight: 600;
    font-size: 10px;
    letter-spacing: 0.08em;
    backdrop-filter: blur(6px);
    pointer-events: none;
    animation: fade-in-hint 0.4s ease forwards;
  }

  @keyframes fade-in-hint {
    from { opacity: 0; transform: translateY(4px); }
    to   { opacity: 1; transform: translateY(0); }
  }

  .exif-panel {
    height: 72px;
    display: flex;
    align-items: center;
    justify-content: space-around;
    padding: 0 24px;
    border-radius: 12px;
    margin-top: 20px;
  }

  .exif-item {
    display: flex;
    flex-direction: column;
    align-items: center;
  }

  .exif-label {
    font-size: 9px;
    font-weight: 700;
    color: hsl(var(--text-muted));
    letter-spacing: 0.1em;
    margin-bottom: 4px;
  }

  .exif-val {
    font-family: var(--font-display);
    font-size: 13px;
    font-weight: 600;
    color: hsl(var(--text-primary));
  }

  .exif-divider {
    width: 1px;
    height: 32px;
    background: hsl(var(--border-muted));
  }

  /* Progress widgets and loaders */
  .sidebar-footer {
    padding: 16px 20px;
    border-top: 1px solid hsl(var(--border-muted));
  }

  .finish-btn {
    width: 100%;
    justify-content: center;
  }

  .welcome-container {
    max-width: 460px;
    width: 100%;
    padding: 40px 36px;
    border-radius: 16px;
    text-align: center;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.6);
  }

  .welcome-logo-container {
    display: flex;
    justify-content: center;
    margin-bottom: 16px;
  }

  .welcome-logo-img {
    width: 64px;
    height: 64px;
    object-fit: contain;
    filter: drop-shadow(0 8px 16px rgba(0, 0, 0, 0.4)) drop-shadow(0 0 10px hsl(var(--accent-amber) / 0.15));
    animation: pulse-glow 3s infinite ease-in-out;
  }

  @keyframes pulse-glow {
    0%, 100% {
      transform: scale(1.0);
      filter: drop-shadow(0 8px 16px rgba(0, 0, 0, 0.4)) drop-shadow(0 0 10px hsl(var(--accent-amber) / 0.15));
    }
    50% {
      transform: scale(1.04);
      filter: drop-shadow(0 12px 24px rgba(0, 0, 0, 0.5)) drop-shadow(0 0 20px hsl(var(--accent-amber) / 0.3));
    }
  }

  /* Apertin wordmark — split color, no gradient */
  .welcome-heading {
    font-family: var(--font-display);
    font-size: 36px;
    font-weight: 800;
    margin-bottom: 4px;
    letter-spacing: -0.04em;
    line-height: 1;
  }

  .wordmark-apt {
    color: hsl(var(--text-primary));
  }

  .wordmark-tin {
    color: hsl(var(--accent-amber));
  }

  .welcome-tagline {
    font-family: var(--font-display);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.22em;
    text-transform: uppercase;
    color: hsl(var(--text-muted));
    margin-bottom: 20px;
  }

  .welcome-byline {
    font-size: 11px;
    font-weight: 500;
    color: hsl(var(--text-muted));
    margin-top: -8px;
    margin-bottom: 20px;
    letter-spacing: 0.01em;
  }

  .welcome-desc {
    color: hsl(var(--text-secondary));
    font-size: 13px;
    margin-bottom: 24px;
    line-height: 1.55;
  }

  .input-group {
    display: flex;
    gap: 12px;
    margin-bottom: 36px;
    background: hsl(var(--bg-darker));
    padding: 6px;
    border-radius: 10px;
    border: 1px solid hsl(var(--border-muted));
  }

  .dir-input {
    flex: 1;
    background: transparent;
    border: none;
    outline: none;
    color: hsl(var(--text-primary));
    padding: 10px 16px;
    font-family: monospace;
    font-size: 12px;
  }

  .scan-btn {
    white-space: nowrap;
  }

  .scan-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .shortcuts-legend {
    border-top: 1px solid hsl(var(--border-muted));
    padding-top: 20px;
    text-align: left;
  }

  .legend-title {
    font-family: var(--font-display);
    font-size: 10px;
    font-weight: 700;
    color: hsl(var(--text-muted));
    letter-spacing: 0.1em;
    margin-bottom: 12px;
    text-align: center;
  }

  .legend-grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 10px 20px;
  }

  .legend-item {
    display: flex;
    flex-direction: column;
    gap: 5px;
    color: hsl(var(--text-secondary));
    font-size: 11px;
  }

  .complete-container {
    max-width: 500px;
    width: 100%;
    padding: 64px 48px;
    border-radius: 12px;
    text-align: center;
    box-shadow: 0 10px 40px rgba(0, 0, 0, 0.5);
  }

  .success-icon {
    width: 80px;
    height: 80px;
    border-radius: 50%;
    background: hsl(var(--accent-keep) / 0.1);
    color: hsl(var(--accent-keep));
    border: 2px solid hsl(var(--accent-keep));
    font-size: 40px;
    display: flex;
    align-items: center;
    justify-content: center;
    margin: 0 auto 28px;
    box-shadow: 0 0 30px var(--accent-keep-glow);
  }

  .complete-heading {
    font-family: var(--font-display);
    font-size: 28px;
    font-weight: 800;
    letter-spacing: -0.02em;
    margin-bottom: 12px;
  }

  .complete-desc {
    color: hsl(var(--text-secondary));
    margin-bottom: 40px;
    line-height: 1.6;
  }

  .complete-buttons {
    display: flex;
    justify-content: center;
  }

  .loading-overlay {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: hsl(var(--bg-darker) / 0.85);
    backdrop-filter: blur(8px);
    z-index: 1000;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 20px;
  }

  .loading-text {
    font-family: var(--font-display);
    font-weight: 600;
    color: hsl(var(--text-secondary));
    font-size: 14px;
    letter-spacing: 0.02em;
  }

  .spinner {
    width: 48px;
    height: 48px;
    border: 3px solid hsl(var(--bg-input));
    border-top: 3px solid hsl(var(--accent-amber));
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    box-shadow: 0 0 15px var(--accent-amber-glow);
  }

  @keyframes spin {
    0% { transform: rotate(0deg); }
    100% { transform: rotate(360deg); }
  }

  /* Quick metrics fixes */
  .badge {
    background: hsl(var(--bg-input));
    padding: 2px 8px;
    border-radius: 99px;
    font-size: 10px;
    color: hsl(var(--text-primary));
  }

  .progress-bar-container {
    width: 100%;
    height: 4px;
    background: hsl(var(--bg-input));
    border-radius: 2px;
    overflow: hidden;
  }

  .progress-bar {
    height: 100%;
    background: hsl(var(--accent-amber));
    border-radius: 2px;
    transition: width 0.3s cubic-bezier(0.4, 0, 0.2, 1);
  }

  .stats-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 8px;
    padding: 16px 20px;
    border-bottom: 1px solid hsl(var(--border-muted));
  }

  .stat-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 10px 4px;
    background: hsl(var(--bg-input) / 0.5);
    border: 1px solid hsl(var(--border-muted));
    border-radius: 6px;
  }

  .stat-num {
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 800;
  }

  .stat-card.keep .stat-num { color: hsl(var(--accent-keep)); }
  .stat-card.trash .stat-num { color: hsl(var(--accent-trash)); }
  .stat-card.star .stat-num { color: hsl(var(--accent-star)); }

  .stat-label {
    font-size: 10px;
    color: hsl(var(--text-secondary));
    margin-top: 2px;
  }

  .file-list-title {
    padding: 16px 20px 8px;
    font-size: 11px;
    font-weight: 700;
    color: hsl(var(--text-muted));
    letter-spacing: 0.05em;
  }

  .file-list {
    flex: 1;
    overflow-y: auto;
    padding: 0 16px 24px;
  }

  .file-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 8px 12px;
    margin-bottom: 4px;
    border-radius: 6px;
    background: transparent;
    text-align: left;
    color: hsl(var(--text-secondary));
    border: 1px solid transparent;
    transition: all 0.15s ease;
  }

  .file-item:hover {
    background: hsl(var(--bg-input) / 0.5);
    color: hsl(var(--text-primary));
  }

  .file-item.active {
    background: hsl(var(--bg-input));
    color: hsl(var(--text-primary));
    border-color: hsl(var(--accent-amber) / 0.3);
  }

  .file-info-left {
    display: flex;
    align-items: center;
    gap: 8px;
    overflow: hidden;
  }

  .file-idx {
    font-size: 10px;
    color: hsl(var(--text-muted));
    font-weight: 600;
    min-width: 16px;
  }

  .file-name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 12px;
  }

  .file-badges {
    display: flex;
    gap: 4px;
  }

  .item-badge {
    font-size: 10px;
    font-weight: bold;
    width: 14px;
    height: 14px;
    border-radius: 3px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .item-badge.keep { background: hsl(var(--accent-keep) / 0.2); color: hsl(var(--accent-keep)); }
  .item-badge.trash { background: hsl(var(--accent-trash) / 0.2); color: hsl(var(--accent-trash)); }
  .item-badge.star { background: hsl(var(--accent-star) / 0.2); color: hsl(var(--accent-star)); }
</style>
