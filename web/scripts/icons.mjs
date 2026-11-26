// Renders the Strata app icons (stacked layers) to PNG with no dependencies.
import { deflateSync } from 'node:zlib';
import { writeFileSync, mkdirSync } from 'node:fs';

const crcTable = new Uint32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (buf) => {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
};

function png(size, pixel) {
  const raw = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0;
    for (let x = 0; x < size; x++) {
      // 4x supersampling for smooth edges
      let r = 0, g = 0, b = 0, a = 0;
      for (let sy = 0; sy < 4; sy++)
        for (let sx = 0; sx < 4; sx++) {
          const p = pixel((x + (sx + 0.5) / 4) / size, (y + (sy + 0.5) / 4) / size);
          r += p[0] * p[3]; g += p[1] * p[3]; b += p[2] * p[3]; a += p[3];
        }
      const o = y * (size * 4 + 1) + 1 + x * 4;
      raw[o] = a ? r / a : 0; raw[o + 1] = a ? g / a : 0; raw[o + 2] = a ? b / a : 0; raw[o + 3] = (a / 16) * 255;
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; ihdr[9] = 6; ihdr[10] = 0; ihdr[11] = 0; ihdr[12] = 0;
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0))
  ]);
}

const BG = [15, 23, 42];
const LAYERS = [
  [94, 234, 212],
  [56, 189, 248],
  [129, 140, 248]
];

function roundedRect(x, y, x0, y0, x1, y1, r) {
  const cx = Math.min(Math.max(x, x0 + r), x1 - r);
  const cy = Math.min(Math.max(y, y0 + r), y1 - r);
  return (x - cx) ** 2 + (y - cy) ** 2 <= r * r && x >= x0 && x <= x1 && y >= y0 && y <= y1;
}

// `pad`: 0 for full-bleed (maskable / apple), >0 leaves a transparent margin.
function icon(pad, rounded) {
  return (x, y) => {
    const inBg = rounded ? roundedRect(x, y, pad, pad, 1 - pad, 1 - pad, 0.22) : true;
    if (!inBg) return [0, 0, 0, 0];
    // three slightly skewed bands, like rock strata
    for (let i = 0; i < 3; i++) {
      const top = 0.3 + i * 0.15 + (x - 0.5) * 0.08;
      if (roundedRect(x, y, 0.24, top, 0.76, top + 0.1, 0.05)) return [...LAYERS[i], 1];
    }
    return [...BG, 1];
  };
}

/** A launch screen: the background with the (full-bleed) icon in the
 *  middle, a third of the short side. Opaque RGB; only the icon's square is
 *  supersampled. */
function splash(w, h, [br, bg, bb]) {
  const s = Math.round(Math.min(w, h) / 3);
  const ix = Math.round((w - s) / 2), iy = Math.round((h - s) / 2);
  const draw = icon(0, true);
  const row = Buffer.alloc(1 + w * 3);
  const raw = Buffer.alloc((1 + w * 3) * h);
  for (let y = 0; y < h; y++) {
    row[0] = 0;
    for (let x = 0; x < w; x++) {
      let r = br, g = bg, b = bb;
      if (x >= ix && x < ix + s && y >= iy && y < iy + s) {
        let ar = 0, ag = 0, ab = 0, aa = 0;
        for (let sy = 0; sy < 4; sy++)
          for (let sx = 0; sx < 4; sx++) {
            const p = draw((x - ix + (sx + 0.5) / 4) / s, (y - iy + (sy + 0.5) / 4) / s);
            ar += p[0] * p[3]; ag += p[1] * p[3]; ab += p[2] * p[3]; aa += p[3];
          }
        const k = aa / 16;
        if (aa) { r = (ar / aa) * k + br * (1 - k); g = (ag / aa) * k + bg * (1 - k); b = (ab / aa) * k + bb * (1 - k); }
      }
      const o = 1 + x * 3;
      row[o] = r; row[o + 1] = g; row[o + 2] = b;
    }
    row.copy(raw, y * row.length);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; ihdr[9] = 2;
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0))
  ]);
}

/** iPhone launch screens (portrait, device pixels), as listed in
 *  src/app.html: [width, height, CSS width, CSS height, pixel ratio]. */
export const SPLASH = [
  [1320, 2868, 440, 956, 3],
  [1206, 2622, 402, 874, 3],
  [1290, 2796, 430, 932, 3],
  [1179, 2556, 393, 852, 3],
  [1284, 2778, 428, 926, 3],
  [1170, 2532, 390, 844, 3],
  [1125, 2436, 375, 812, 3],
  [1242, 2688, 414, 896, 3],
  [828, 1792, 414, 896, 2],
  [750, 1334, 375, 667, 2],
  [640, 1136, 320, 568, 2]
];

mkdirSync('static/icons', { recursive: true });
writeFileSync('static/icons/icon-192.png', png(192, icon(0.02, true)));
writeFileSync('static/icons/icon-512.png', png(512, icon(0.02, true)));
writeFileSync('static/icons/maskable-512.png', png(512, icon(0, false)));
writeFileSync('static/icons/apple-touch-icon.png', png(180, icon(0, false)));
writeFileSync('static/favicon.png', png(32, icon(0, true)));
mkdirSync('static/splash', { recursive: true });
for (const [w, h] of SPLASH) {
  writeFileSync(`static/splash/light-${w}x${h}.png`, splash(w, h, [248, 250, 252]));
  writeFileSync(`static/splash/dark-${w}x${h}.png`, splash(w, h, [11, 17, 32]));
}
console.log('icons written');
