// Renders the drivecap application icon (a drive in front of a gauge whose
// needle sits in the red) and packs it into the exe/MSI icon file.
//
//   node scripts/app-icon.mjs
//
// Writes tmp/icon/icon-<size>.png, a preview sheet tmp/icon/sheet.png (every
// size on a light and a dark strip, small sizes also at 4x nearest-neighbour)
// and crates/drivecap-cli/assets/drivecap.ico (all sizes, PNG-compressed,
// 32-bpp). Shapes are signed-distance functions, supersampled for
// anti-aliasing. Sizes <= 24 px use a simplified, pixel-aligned design rather
// than a downscale of the full one. Node built-ins only.

import fs from 'node:fs';
import zlib from 'node:zlib';

const ROOT = new URL('../', import.meta.url);
const SIZES = [16, 20, 24, 32, 40, 48, 64, 256];

// ---------- colour + math helpers ----------

const hex = (h) => [parseInt(h.slice(1, 3), 16), parseInt(h.slice(3, 5), 16), parseInt(h.slice(5, 7), 16)];
const mix = (a, b, t) => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
const scale = (c, k) => [c[0] * k, c[1] * k, c[2] * k];
const clamp01 = (x) => Math.max(0, Math.min(1, x));
const smooth = (e0, e1, x) => {
  const t = clamp01((x - e0) / (e1 - e0));
  return t * t * (3 - 2 * t);
};
function ramp(stops, t) {
  t = clamp01(t);
  for (let i = 1; i < stops.length; i++) {
    if (t <= stops[i][0]) {
      const [t0, c0] = stops[i - 1];
      const [t1, c1] = stops[i];
      return mix(c0, c1, (t - t0) / (t1 - t0));
    }
  }
  return stops[stops.length - 1][1];
}

// ---------- signed distance functions (negative = inside) ----------

const sdCircle = (x, y, cx, cy, r) => Math.hypot(x - cx, y - cy) - r;
function sdRoundRect(x, y, x0, y0, x1, y1, r) {
  const cx = (x0 + x1) / 2, cy = (y0 + y1) / 2;
  const qx = Math.abs(x - cx) - ((x1 - x0) / 2 - r);
  const qy = Math.abs(y - cy) - ((y1 - y0) / 2 - r);
  return Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - r;
}
function sdPolygon(x, y, pts) {
  let d = Infinity, s = 1;
  for (let i = 0, j = pts.length - 1; i < pts.length; j = i++) {
    const [ax, ay] = pts[j], [bx, by] = pts[i];
    const ex = bx - ax, ey = by - ay, wx = x - ax, wy = y - ay;
    const h = clamp01((wx * ex + wy * ey) / (ex * ex + ey * ey));
    d = Math.min(d, Math.hypot(wx - ex * h, wy - ey * h));
    const c1 = y >= ay, c2 = y < by, c3 = ex * wy > ey * wx;
    if ((c1 && c2 && c3) || (!c1 && !c2 && !c3)) s = -s;
  }
  return s * d;
}
// Tapered segment a->b with radius ra at a and rb at b.
function sdTaper(x, y, ax, ay, bx, by, ra, rb) {
  const ex = bx - ax, ey = by - ay, wx = x - ax, wy = y - ay;
  const h = clamp01((wx * ex + wy * ey) / (ex * ex + ey * ey));
  return Math.hypot(wx - ex * h, wy - ey * h) - (ra + (rb - ra) * h);
}

// ---------- palette ----------

const P = {
  green: hex('#3fae4a'), green2: hex('#7cc242'), yellow: hex('#f6c623'), orange: hex('#f39221'),
  orange2: hex('#ec6a25'), red: hex('#e2402f'), red2: hex('#c92a22'),
  bezelTop: hex('#fdfdfe'), bezelBot: hex('#d3d7dd'), bezelRim: hex('#a7adb6'),
  dialTop: hex('#ffffff'), dialBot: hex('#e7eaee'),
  ink: hex('#2a2f38'), inkHi: hex('#4d5563'), inkLo: hex('#171a20'),
  topBack: hex('#f4f5f7'), topFront: hex('#cfd3d9'), frameTop: hex('#b9bec6'), frameBot: hex('#8f959e'),
  panelTop: hex('#3b424d'), panelBot: hex('#1d2026'), outline: hex('#6f7580'),
  led: hex('#4fe04f'), ledDeep: hex('#21a630'),
};

// Painter's-algorithm compositing into a premultiplied accumulator.
function over(acc, rgb, a) {
  if (a <= 0) return;
  const k = 1 - a;
  acc[0] = rgb[0] * a + acc[0] * k;
  acc[1] = rgb[1] * a + acc[1] * k;
  acc[2] = rgb[2] * a + acc[2] * k;
  acc[3] = a + acc[3] * k;
}

// ---------- full / moderate detail (32 px and up), unit coordinates ----------

function sceneFull(S) {
  const px = 1 / S; // one output pixel in unit space
  const mid = S < 128; // moderate detail
  const C = [0.5, 0.52];
  const RB = 0.475; // bezel radius
  const R0 = mid ? 0.285 : 0.3, R1 = 0.425; // colour band
  const A0 = 196, A1 = -16; // band sweep, degrees (math convention, y up)
  const gap = mid ? 0 : 1.4; // degrees between colour segments
  const rim = Math.max(0.011, 1.0 * px);
  const needleDeg = 33;
  const nd = (needleDeg * Math.PI) / 180;
  const tip = [C[0] + Math.cos(nd) * 0.355, C[1] - Math.sin(nd) * 0.355];
  const tail = [C[0] - Math.cos(nd) * 0.05, C[1] + Math.sin(nd) * 0.05];
  const hubR = mid ? 0.075 : 0.068;

  // Drive geometry.
  const yBack = 0.585, yEdge = 0.705, yBot = 0.935;
  const xBackL = 0.235, xBackR = 0.765, xL = 0.13, xR = 0.87;
  const body = [[xBackL, yBack], [xBackR, yBack], [xR, yEdge], [xR, yBot], [xL, yBot], [xL, yEdge]];
  const bodyR = 0.03;
  const panel = [xL + 0.04, yEdge + 0.032, xR - 0.04, yBot - 0.038];
  const led = [panel[2] - 0.07, (panel[1] + panel[3]) / 2, mid ? 0.03 : 0.026];

  const segStops = [
    [0, P.green], [0.18, P.green2], [0.4, P.yellow], [0.6, P.orange], [0.74, P.orange2], [0.86, P.red], [1, P.red2],
  ];

  return (x, y) => {
    const acc = [0, 0, 0, 0];

    // Shadow under the drive.
    {
      const dx = (x - 0.5) / 0.42, dy = (y - 0.945) / 0.045;
      const d = Math.hypot(dx, dy);
      over(acc, [0, 0, 0], 0.32 * (1 - smooth(0.3, 1, d)));
    }

    // Bezel: half disc + straight sides down behind the drive.
    const dBez = y <= C[1] ? sdCircle(x, y, C[0], C[1], RB) : sdRoundRect(x, y, C[0] - RB, C[1] - 1, C[0] + RB, 0.8, 0.07);
    if (dBez < 0) {
      over(acc, mix(P.bezelTop, P.bezelBot, smooth(0.05, 0.8, y)), 1);
      if (dBez > -rim) over(acc, P.bezelRim, 1);
      else if (!mid && dBez > -rim - 0.012) over(acc, [255, 255, 255], 0.7); // inner highlight
    }

    // Dial face inside the band.
    const r = Math.hypot(x - C[0], y - C[1]);
    if (r < R0 && y < C[1] + 0.12) {
      over(acc, mix(P.dialTop, P.dialBot, smooth(C[1] - R0, C[1] + 0.1, y)), 1);
    }

    // Colour band.
    let ang = (Math.atan2(C[1] - y, x - C[0]) * 180) / Math.PI;
    if (ang < -90) ang += 360;
    const t = (A0 - ang) / (A0 - A1);
    if (r >= R0 && r <= R1 && t >= 0 && t <= 1) {
      // Four segments separated by thin radial gaps.
      const seg = t * 4;
      const nearest = Math.min(3, Math.max(1, Math.round(seg)));
      const isGap = gap > 0 && Math.abs(seg - nearest) < ((gap / (A0 - A1)) * 4) / 2;
      if (!isGap) {
        let c = ramp(segStops, t);
        const rr = (r - R0) / (R1 - R0); // 0 inner .. 1 outer
        c = scale(c, 0.9 + 0.14 * rr);
        c = mix(c, [255, 255, 255], 0.12 * smooth(0.6, 0.0, y)); // lighter toward the top
        over(acc, c, 1);
        if (!mid && r < R0 + 0.008) over(acc, [0, 0, 0], 0.12); // inner edge depth
      }
    }

    // Needle with a faint drop shadow.
    if (!mid) {
      const ds = sdTaper(x, y, tail[0] + 0.008, tail[1] + 0.012, tip[0] + 0.008, tip[1] + 0.012, 0.03, 0.013);
      over(acc, [0, 0, 0], 0.18 * (1 - smooth(-0.004, 0.012, ds)));
    }
    const dn = sdTaper(x, y, tail[0], tail[1], tip[0], tip[1], mid ? 0.036 : 0.03, mid ? 0.019 : 0.013);
    if (dn < 0) {
      // Lit along the upper-left edge.
      const nx = -Math.sin(nd), ny = -Math.cos(nd);
      const side = (x - C[0]) * nx + (y - C[1]) * ny;
      over(acc, side > 0.004 ? P.inkHi : P.ink, 1);
    }

    // Hub.
    const dh = sdCircle(x, y, C[0], C[1], hubR);
    if (dh < 0) {
      over(acc, mix(P.inkHi, P.inkLo, smooth(C[1] - hubR, C[1] + hubR, y)), 1);
      if (!mid) {
        const dg = sdCircle(x, y, C[0] - hubR * 0.3, C[1] - hubR * 0.35, hubR * 0.35);
        over(acc, [255, 255, 255], 0.22 * (1 - smooth(-0.02, 0.01, dg)));
      }
    }

    // Drive body.
    const db = sdPolygon(x, y, body) - bodyR;
    if (db < 0) {
      if (y < yEdge) over(acc, mix(P.topBack, P.topFront, smooth(yBack - 0.03, yEdge, y)), 1);
      else over(acc, mix(P.frameTop, P.frameBot, smooth(yEdge, yBot, y)), 1);
      if (y >= yEdge - Math.max(0.006, px) && y < yEdge + Math.max(0.006, px) * (mid ? 0 : 1)) {
        over(acc, [255, 255, 255], 0.75); // bevel between top and front
      }
      if (db > -Math.max(0.009, px)) over(acc, P.outline, 0.85);
      const dp = sdRoundRect(x, y, ...panel, 0.032);
      if (dp < 0) {
        over(acc, mix(P.panelTop, P.panelBot, smooth(panel[1], panel[3], y)), 1);
        if (!mid && dp > -0.007 && y > (panel[1] + panel[3]) / 2) over(acc, [255, 255, 255], 0.1);
      }
      // LED with glow.
      const dl = sdCircle(x, y, led[0], led[1], led[2]);
      if (dp < 0) over(acc, P.led, (mid ? 0.25 : 0.35) * (1 - smooth(0, led[2] * 1.4, dl)));
      if (dl < 0) {
        over(acc, mix(P.led, P.ledDeep, smooth(led[1] - led[2], led[1] + led[2], y)), 1);
        if (!mid) {
          const dsp = sdCircle(x, y, led[0] - led[2] * 0.3, led[1] - led[2] * 0.35, led[2] * 0.35);
          over(acc, [255, 255, 255], 0.75 * (1 - smooth(-0.004, 0.003, dsp)));
        }
      }
    }
    return acc;
  };
}

// ---------- simplified small sizes (16-24 px), pixel coordinates ----------

function sceneSmall(S) {
  const k = S / 16;
  const R = (v) => Math.round(v * k); // pixel-aligned edge
  const C = [S / 2, R(8.5)];
  const RO = 7.6 * k, RI = 4.1 * k;
  const segs = [P.green, P.yellow, P.orange, P.red];
  const nd = (36 * Math.PI) / 180;
  const tip = [C[0] + Math.cos(nd) * 6.6 * k, C[1] - Math.sin(nd) * 6.6 * k];
  const top = [[R(3), R(9)], [S - R(3), R(9)], [S - R(1), R(11)], [R(1), R(11)]];
  const front = [R(1), R(11), S - R(1), R(15.5)];
  const led = [S - R(4), R(12.5), S - R(2), R(14)];

  return (x, y) => {
    const acc = [0, 0, 0, 0];
    const r = Math.hypot(x - C[0], y - C[1]);
    if (y <= C[1] && r < RO) {
      if (r < RI) over(acc, P.dialBot, 1);
      else {
        const ang = (Math.atan2(C[1] - y, x - C[0]) * 180) / Math.PI; // 0..180
        over(acc, segs[Math.min(3, Math.floor((180 - ang) / 45))], 1);
      }
    }
    if (sdTaper(x, y, C[0], C[1], tip[0], tip[1], 1.0 * k, 0.75 * k) < 0) over(acc, P.inkLo, 1);
    if (sdCircle(x, y, C[0], C[1], 1.9 * k) < 0) over(acc, P.inkLo, 1);
    if (sdPolygon(x, y, top) < 0) over(acc, hex('#d9dde3'), 1);
    if (sdRoundRect(x, y, ...front, 1.0 * k) < 0) over(acc, P.ink, 1);
    if (x >= led[0] && x < led[2] && y >= led[1] && y < led[3]) over(acc, P.led, 1);
    return acc;
  };
}

// ---------- rasterise ----------

function render(S) {
  const small = S <= 24;
  const shade = small ? sceneSmall(S) : sceneFull(S);
  const n = small ? 8 : S >= 128 ? 4 : 6;
  const out = new Uint8Array(S * S * 4);
  for (let py = 0; py < S; py++) {
    for (let px = 0; px < S; px++) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let j = 0; j < n; j++) {
        for (let i = 0; i < n; i++) {
          const sx = px + (i + 0.5) / n, sy = py + (j + 0.5) / n;
          const c = small ? shade(sx, sy) : shade(sx / S, sy / S);
          r += c[0]; g += c[1]; b += c[2]; a += c[3];
        }
      }
      const o = (py * S + px) * 4;
      const nn = n * n;
      a /= nn;
      if (a > 0) {
        out[o] = Math.round(Math.min(255, r / nn / a));
        out[o + 1] = Math.round(Math.min(255, g / nn / a));
        out[o + 2] = Math.round(Math.min(255, b / nn / a));
      }
      out[o + 3] = Math.round(a * 255);
    }
  }
  return { w: S, h: S, px: out };
}

// ---------- PNG / ICO encoding ----------

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
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}
function png({ w, h, px }) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  const raw = Buffer.alloc((w * 4 + 1) * h);
  for (let y = 0; y < h; y++) Buffer.from(px.buffer, px.byteOffset + y * w * 4, w * 4).copy(raw, y * (w * 4 + 1) + 1);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}
function ico(entries) {
  const head = Buffer.alloc(6 + 16 * entries.length);
  head.writeUInt16LE(0, 0); // reserved
  head.writeUInt16LE(1, 2); // type: icon
  head.writeUInt16LE(entries.length, 4);
  let offset = head.length;
  entries.forEach(({ size, data }, i) => {
    const e = 6 + 16 * i;
    head[e] = size >= 256 ? 0 : size; // width (0 = 256)
    head[e + 1] = size >= 256 ? 0 : size; // height
    head[e + 2] = 0; // palette colours
    head[e + 3] = 0; // reserved
    head.writeUInt16LE(1, e + 4); // planes
    head.writeUInt16LE(32, e + 6); // bits per pixel
    head.writeUInt32LE(data.length, e + 8);
    head.writeUInt32LE(offset, e + 12);
    offset += data.length;
  });
  return Buffer.concat([head, ...entries.map((e) => e.data)]);
}

// ---------- preview sheet ----------

function blit(dst, src, ox, oy, zoom = 1) {
  for (let y = 0; y < src.h * zoom; y++) {
    for (let x = 0; x < src.w * zoom; x++) {
      const s = (Math.floor(y / zoom) * src.w + Math.floor(x / zoom)) * 4;
      const d = ((oy + y) * dst.w + ox + x) * 4;
      const a = src.px[s + 3] / 255;
      for (let c = 0; c < 3; c++) dst.px[d + c] = Math.round(src.px[s + c] * a + dst.px[d + c] * (1 - a));
    }
  }
}
function sheet(images) {
  const pad = 16;
  const zoomed = images.filter((im) => im.w <= 48);
  const row1W = images.reduce((s, im) => s + im.w + pad, pad);
  const row2W = zoomed.reduce((s, im) => s + im.w * 4 + pad, pad);
  const stripH = pad + 256 + pad + 48 * 4 + pad;
  const w = Math.max(row1W, row2W), h = stripH * 2;
  const out = { w, h, px: new Uint8Array(w * h * 4) };
  ['#f3f3f3', '#202020'].forEach((bg, s) => {
    const c = hex(bg);
    for (let y = s * stripH; y < (s + 1) * stripH; y++) {
      for (let x = 0; x < w; x++) out.px.set([...c, 255], (y * w + x) * 4);
    }
    let x = pad;
    for (const im of images) {
      blit(out, im, x, s * stripH + pad + 256 - im.h);
      x += im.w + pad;
    }
    x = pad;
    for (const im of zoomed) {
      blit(out, im, x, s * stripH + pad + 256 + pad, 4);
      x += im.w * 4 + pad;
    }
  });
  return out;
}

// ---------- main ----------

const tmpDir = new URL('tmp/icon/', ROOT);
const assetDir = new URL('crates/drivecap-cli/assets/', ROOT);
fs.mkdirSync(tmpDir, { recursive: true });
fs.mkdirSync(assetDir, { recursive: true });

const images = SIZES.map(render);
const entries = images.map((im) => ({ size: im.w, data: png(im) }));
entries.forEach(({ size, data }) => fs.writeFileSync(new URL(`icon-${size}.png`, tmpDir), data));
fs.writeFileSync(new URL('sheet.png', tmpDir), png(sheet(images)));
const icoBuf = ico(entries);
fs.writeFileSync(new URL('drivecap.ico', assetDir), icoBuf);
console.log(`drivecap.ico: ${SIZES.join(', ')} px, ${icoBuf.length} bytes`);
