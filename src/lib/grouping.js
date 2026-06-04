// Pure helpers for the Smart Grouping feature.
// Kept free of component state so they can be unit-tested and reused.

// Time gap presets (seconds) for time-based grouping.
export const TIME_GAPS = [
  { label: '30s', value: 30 },
  { label: '2 min', value: 120 },
  { label: '5 min', value: 300 },
];

// Visual similarity thresholds — Hamming distance over the 64-bit pHash.
//   6  = burst / near-identical only
//   10 = same scene, varying exposure/framing (recommended default)
//   15 = similar subject, different angle
export const THRESHOLDS = [
  { label: 'Burst', value: 6 },
  { label: 'Normal', value: 10 },
  { label: 'Loose', value: 15 },
];

const GROUP_COLORS = [
  '#f59e0b', '#3b82f6', '#10b981', '#8b5cf6', '#ef4444',
  '#06b6d4', '#f97316', '#84cc16', '#ec4899', '#14b8a6',
  '#6366f1', '#a78bfa',
];

export function groupColor(gid) {
  return gid == null ? 'transparent' : GROUP_COLORS[gid % GROUP_COLORS.length];
}

// Parse EXIF date "2026:06:02 12:30:15" → ms timestamp (or null).
export function parseExifDate(str) {
  if (!str) return null;
  const iso = str.replace(/^(\d{4}):(\d{2}):(\d{2})/, '$1-$2-$3');
  const t = Date.parse(iso);
  return isNaN(t) ? null : t;
}

// Instant time-based grouping (pure JS, no Rust call). Returns an array of
// group ids (or null) aligned with `files`.
export function groupByTime(files, gapSec) {
  const gapMs = gapSec * 1000;
  const assignments = new Array(files.length).fill(null);
  const indexed = files
    .map((f, i) => ({ i, t: parseExifDate(f.date_time) }))
    .filter((x) => x.t != null)
    .sort((a, b) => a.t - b.t);

  let gid = 0;
  let lastT = null;
  for (const { i, t } of indexed) {
    if (lastT == null || t - lastT > gapMs) gid++;
    assignments[i] = gid;
    lastT = t;
  }
  return assignments;
}

// Count how many files belong to each group id.
export function computeGroupSizes(groupAssignments) {
  const s = {};
  for (const gid of groupAssignments) {
    if (gid != null) s[gid] = (s[gid] || 0) + 1;
  }
  return s;
}

// Re-order indices so all members of a group render consecutively, anchored at
// the position of the group's earliest member.
export function computeDisplayOrder(files, groupAssignments, groupSizes) {
  const placed = new Set();
  const order = [];
  for (let i = 0; i < files.length; i++) {
    if (placed.has(i)) continue;
    const gid = groupAssignments[i] ?? null;
    if (gid != null && (groupSizes[gid] ?? 1) > 1) {
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
}
