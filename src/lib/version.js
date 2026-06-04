// Semantic-version helpers + GitHub release lookup for the update checker.

// Extract a clean "x.y.z" from an arbitrary version/tag string.
export function normalizeVersion(str) {
  if (!str) return '';
  const m = String(str).match(/(\d+)\.(\d+)\.(\d+)/);
  return m ? `${m[1]}.${m[2]}.${m[3]}` : '';
}

// Numeric semver comparison: returns true when `latest` is strictly newer than
// `current`. Avoids the classic "0.10.0" < "0.9.0" string-compare bug.
export function isNewerVersion(latest, current) {
  const a = normalizeVersion(latest).split('.').map(Number);
  const b = normalizeVersion(current).split('.').map(Number);
  if (a.length < 3 || b.length < 3) return false;
  for (let i = 0; i < 3; i++) {
    if (a[i] > b[i]) return true;
    if (a[i] < b[i]) return false;
  }
  return false;
}

// Fetch the latest GitHub release for a "owner/repo". Resolves with
// { version, url } or null on any failure.
export async function fetchLatestRelease(repo) {
  const res = await fetch(`https://api.github.com/repos/${repo}/releases/latest`, {
    headers: { Accept: 'application/vnd.github+json' },
  });
  if (!res.ok) throw new Error('network');
  const data = await res.json();
  const version = normalizeVersion(data.tag_name);
  if (!version) return null;
  return { version, url: data.html_url ?? '' };
}
