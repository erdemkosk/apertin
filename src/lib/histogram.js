// Pure-ish helpers for the EXIF panel histogram.

// Compute RGB + luma histograms from an image URL. Resolves with
// { r, g, b, l, max } (Uint32Array bins + peak count) or null on failure.
// Analyses a downscaled 128×128 sample for speed.
export function computeHistogram(imgUrl) {
  return new Promise((resolve) => {
    if (!imgUrl) {
      resolve(null);
      return;
    }
    const img = new Image();
    img.crossOrigin = 'anonymous';
    img.onload = () => {
      const canvas = document.createElement('canvas');
      const ctx = canvas.getContext('2d');
      canvas.width = 128;
      canvas.height = 128;
      ctx.drawImage(img, 0, 0, 128, 128);

      const data = ctx.getImageData(0, 0, 128, 128).data;
      const l = new Uint32Array(256);
      const r = new Uint32Array(256);
      const g = new Uint32Array(256);
      const b = new Uint32Array(256);

      for (let i = 0; i < data.length; i += 4) {
        const rv = data[i];
        const gv = data[i + 1];
        const bv = data[i + 2];
        const lv = Math.round(0.299 * rv + 0.587 * gv + 0.114 * bv);
        r[rv]++;
        g[gv]++;
        b[bv]++;
        l[lv]++;
      }

      let max = 0;
      for (let i = 0; i < 256; i++) {
        if (r[i] > max) max = r[i];
        if (g[i] > max) max = g[i];
        if (b[i] > max) max = b[i];
        if (l[i] > max) max = l[i];
      }
      resolve({ r, g, b, l, max });
    };
    img.onerror = () => resolve(null);
    img.src = imgUrl;
  });
}

// Build an SVG path string for one histogram channel (200×80 viewport).
export function generateSvgPath(data, max) {
  if (max === 0) return 'M 0 80';
  let path = 'M 0 80';
  for (let i = 0; i < 256; i++) {
    const x = (i / 255) * 200;
    const y = 80 - (data[i] / max) * 75;
    path += ` L ${x} ${y}`;
  }
  path += ' L 200 80 Z';
  return path;
}
