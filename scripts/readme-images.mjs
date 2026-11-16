// Turns the real painter's renders into the README images.
//
//   cargo test -p drivecap-cli --lib -- --ignored dump_readme_images && node scripts/readme-images.mjs
//
// The ignored `dump_readme_images` tests (paint.rs, popup_wnd/render.rs,
// tray.rs) write `tmp/readme/<name>.rgba`: width u32 LE, height u32 LE,
// then straight RGBA8 rows. This script writes `docs/images/<name>.png`
// (cropped to the visible pixels) plus two composites, `hero.png` and
// `tray-icons.png`. Node built-ins only.

import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname.replace(/^\/(\w:)/, '$1')), '..');
const src = path.join(root, 'tmp', 'readme');
const out = path.join(root, 'docs', 'images');

// ---------- images ----------

function image(w, h) {
  return { w, h, px: new Uint8Array(w * h * 4) };
}

function readRgba(file) {
  const buf = fs.readFileSync(file);
  const w = buf.readUInt32LE(0);
  const h = buf.readUInt32LE(4);
  if (buf.length !== 8 + w * h * 4) throw new Error(`${file}: bad size`);
  return { w, h, px: new Uint8Array(buf.subarray(8)) };
}

/** Crops to the bounding box of pixels with any alpha. */
function cropVisible(img) {
  let [x0, y0, x1, y1] = [img.w, img.h, -1, -1];
  for (let y = 0; y < img.h; y++) {
    for (let x = 0; x < img.w; x++) {
      if (img.px[(y * img.w + x) * 4 + 3]) {
        x0 = Math.min(x0, x); y0 = Math.min(y0, y);
        x1 = Math.max(x1, x); y1 = Math.max(y1, y);
      }
    }
  }
  if (x1 < 0) return img;
  const c = image(x1 - x0 + 1, y1 - y0 + 1);
  for (let y = 0; y < c.h; y++) {
    const from = ((y + y0) * img.w + x0) * 4;
    c.px.set(img.px.subarray(from, from + c.w * 4), y * c.w * 4);
  }
  return c;
}

/** Straight-alpha source-over of `s` onto `d` at (ox, oy). */
function over(d, s, ox, oy) {
  for (let y = 0; y < s.h; y++) {
    const dy = y + oy;
    if (dy < 0 || dy >= d.h) continue;
    for (let x = 0; x < s.w; x++) {
      const dx = x + ox;
      if (dx < 0 || dx >= d.w) continue;
      const si = (y * s.w + x) * 4;
      const di = (dy * d.w + dx) * 4;
      const sa = s.px[si + 3] / 255;
      if (sa === 0) continue;
      const da = d.px[di + 3] / 255;
      const oa = sa + da * (1 - sa);
      for (let k = 0; k < 3; k++) {
        const c = (s.px[si + k] * sa + d.px[di + k] * da * (1 - sa)) / oa;
        d.px[di + k] = Math.round(c);
      }
      d.px[di + 3] = Math.round(oa * 255);
    }
  }
}

function fill(img, [r, g, b]) {
  for (let i = 0; i < img.px.length; i += 4) img.px.set([r, g, b, 255], i);
}

function scaleNearest(img, k) {
  const s = image(img.w * k, img.h * k);
  for (let y = 0; y < s.h; y++) {
    for (let x = 0; x < s.w; x++) {
      const si = (Math.floor(y / k) * img.w + Math.floor(x / k)) * 4;
      s.px.set(img.px.subarray(si, si + 4), (y * s.w + x) * 4);
    }
  }
  return s;
}

/** Soft blurred-rectangle shadow (straight alpha) the size of `img`'s box. */
function shadow(w, h, blur, alpha) {
  const pad = blur * 2;
  const s = image(w + 2 * pad, h + 2 * pad);
  const edge = (t) => 1 / (1 + Math.exp(-t / (blur / 3)));
  for (let y = 0; y < s.h; y++) {
    const fy = Math.min(edge(y - pad), edge(h + pad - y));
    for (let x = 0; x < s.w; x++) {
      const fx = Math.min(edge(x - pad), edge(w + pad - x));
      s.px[(y * s.w + x) * 4 + 3] = Math.round(255 * alpha * fx * fy);
    }
  }
  return { img: s, pad };
}

// ---------- PNG ----------

const CRC = new Uint32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

/** Filtered scanlines: per row, the filter with the smallest abs sum. */
function filtered(img) {
  const stride = img.w * 4;
  const raw = Buffer.alloc((stride + 1) * img.h);
  const zero = new Uint8Array(stride);
  const line = new Uint8Array(stride);
  for (let y = 0; y < img.h; y++) {
    const cur = img.px.subarray(y * stride, (y + 1) * stride);
    const up = y ? img.px.subarray((y - 1) * stride, y * stride) : zero;
    let best = null;
    for (let f = 0; f < 5; f++) {
      let sum = 0;
      for (let i = 0; i < stride; i++) {
        const a = i >= 4 ? cur[i - 4] : 0;
        const b = up[i];
        const c = i >= 4 ? up[i - 4] : 0;
        let p;
        if (f === 0) p = 0;
        else if (f === 1) p = a;
        else if (f === 2) p = b;
        else if (f === 3) p = (a + b) >> 1;
        else {
          const q = a + b - c;
          const pa = Math.abs(q - a), pb = Math.abs(q - b), pc = Math.abs(q - c);
          p = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
        }
        const v = (cur[i] - p) & 0xff;
        line[i] = v;
        sum += v < 128 ? v : 256 - v;
      }
      if (!best || sum < best.sum) best = { f, sum, bytes: Buffer.from(line) };
    }
    raw[y * (stride + 1)] = best.f;
    best.bytes.copy(raw, y * (stride + 1) + 1);
  }
  return raw;
}

function writePng(name, img) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(img.w, 0);
  ihdr.writeUInt32BE(img.h, 4);
  ihdr.set([8, 6, 0, 0, 0], 8); // 8-bit RGBA, deflate, adaptive, no interlace
  const idat = zlib.deflateSync(filtered(img), { level: 9 });
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', idat),
    chunk('IEND', Buffer.alloc(0)),
  ]);
  fs.writeFileSync(path.join(out, `${name}.png`), png);
  console.log(`${name}.png  ${img.w}x${img.h}  ${png.length} B`);
}

// ---------- outputs ----------

fs.mkdirSync(out, { recursive: true });
const shots = {};
for (const f of fs.readdirSync(src).filter((f) => f.endsWith('.rgba')).sort()) {
  const name = f.slice(0, -5);
  const img = readRgba(path.join(src, f));
  shots[name] = name.startsWith('card-') ? cropVisible(img) : img;
  writePng(name, shots[name]);
}

// Hero: a dark desktop with the expanded card (halo) on the right and the
// minimized card plus the settings popup to its left.
{
  const hero = image(1200, 520);
  const [a, b] = [[0x1f, 0x2a, 0x44], [0x3a, 0x2c, 0x4a]];
  for (let y = 0; y < hero.h; y++) {
    for (let x = 0; x < hero.w; x++) {
      const t = (x / hero.w + y / hero.h) / 2;
      const i = (y * hero.w + x) * 4;
      for (let k = 0; k < 3; k++) hero.px[i + k] = Math.round(a[k] + (b[k] - a[k]) * t);
      hero.px[i + 3] = 255;
    }
  }
  const card = shots['card-dark'];
  const mini = shots['card-minimized-dark'];
  const pop = shots['popup-settings-dark'];
  const cardX = hero.w - 90 - card.w;
  over(hero, card, cardX, Math.round((hero.h - card.h) / 2));

  const leftW = Math.max(mini.w, pop.w);
  const leftX = Math.round((cardX - leftW) / 2);
  const gap = 36;
  const stackY = Math.round((hero.h - (mini.h + gap + pop.h)) / 2);
  const place = (img, x, y) => {
    const s = shadow(img.w, img.h, 18, 0.35);
    over(hero, s.img, x - s.pad, y - s.pad + 6);
    over(hero, img, x, y);
  };
  place(mini, leftX + Math.round((leftW - mini.w) / 2), stackY);
  place(pop, leftX + Math.round((leftW - pop.w) / 2), stackY + mini.h + gap);
  writePng('hero', hero);
}

// Tray icons: dark taskbar row over light taskbar row, ×2 nearest.
{
  const names = ['healthy', 'warning', 'dire', 'snoozed'];
  const [k, cell, pad] = [2, 96, 0];
  const icon = 32 * k;
  const sheet = image(names.length * cell + pad * 2, 2 * cell);
  [['dark', [0x20, 0x20, 0x20]], ['light', [0xf3, 0xf3, 0xf3]]].forEach(([theme, bg], row) => {
    const band = image(sheet.w, cell);
    fill(band, bg);
    names.forEach((n, i) => {
      const g = scaleNearest(readRgba(path.join(src, `tray-${n}-${theme}.rgba`)), k);
      over(band, g, pad + i * cell + (cell - icon) / 2, (cell - icon) / 2);
    });
    sheet.px.set(band.px, row * cell * sheet.w * 4);
  });
  writePng('tray-icons', sheet);
}
