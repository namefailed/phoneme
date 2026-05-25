/**
 * gen-tray-icons.mjs
 * Generates the four 32×32 tray state PNGs using only Node built-ins.
 * Each icon is a circle with a state-coloured dot on a transparent background,
 * matching the Catppuccin Mocha palette used by the app.
 *
 * Run: node scripts/gen-tray-icons.mjs
 */

import { createWriteStream } from "fs";
import { deflateSync } from "zlib";
import { fileURLToPath } from "url";
import { dirname, join } from "path";

const __dir = dirname(fileURLToPath(import.meta.url));
const ICONS_DIR = join(__dir, "../src-tauri/icons");

// 32×32 RGBA palette — Catppuccin Mocha tones
const STATES = {
  "tray-idle":         { bg: [30, 30, 46, 220],  dot: [203, 166, 247, 255] },  // mauve
  "tray-recording":    { bg: [30, 30, 46, 220],  dot: [243, 139, 168, 255] },  // red
  "tray-transcribing": { bg: [30, 30, 46, 220],  dot: [137, 180, 250, 255] },  // blue
  "tray-error":        { bg: [30, 30, 46, 220],  dot: [250, 179, 135, 255] },  // peach
};

const W = 32, H = 32;
const CX = 16, CY = 16;
const OUTER_R = 14;   // outer ring radius
const DOT_R   = 6;    // filled centre dot radius

function makePng(bgColor, dotColor) {
  // Build raw RGBA pixel data (filter byte prepended per scanline)
  const rows = [];
  for (let y = 0; y < H; y++) {
    const row = [0]; // filter type None
    for (let x = 0; x < W; x++) {
      const dx = x - CX, dy = y - CY;
      const dist = Math.sqrt(dx * dx + dy * dy);

      // Anti-aliased outer ring (1px wide)
      const ringOuter = OUTER_R + 0.5;
      const ringInner = OUTER_R - 1.5;
      const ringAlpha = Math.max(0, Math.min(1,
        dist < ringInner ? 0 : dist > ringOuter ? 0 :
        dist < ringInner + 1 ? dist - ringInner :
        dist > ringOuter - 1 ? ringOuter - dist : 1
      ));

      // Filled dot at centre
      const dotOuter = DOT_R + 0.5;
      const dotAlpha = Math.max(0, Math.min(1, dotOuter - dist));

      if (dotAlpha > 0.01) {
        const a = Math.round(dotColor[3] * dotAlpha);
        row.push(dotColor[0], dotColor[1], dotColor[2], a);
      } else if (ringAlpha > 0.01) {
        const a = Math.round(dotColor[3] * ringAlpha * 0.7);
        row.push(dotColor[0], dotColor[1], dotColor[2], a);
      } else {
        row.push(0, 0, 0, 0); // transparent
      }
    }
    rows.push(Buffer.from(row));
  }
  const raw = Buffer.concat(rows);
  const compressed = deflateSync(raw, { level: 9 });

  // PNG chunks
  const chunks = [];

  // Signature
  chunks.push(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]));

  // IHDR
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(W, 0);
  ihdr.writeUInt32BE(H, 4);
  ihdr[8] = 8;  // bit depth
  ihdr[9] = 6;  // colour type: RGBA
  ihdr[10] = 0; // compression
  ihdr[11] = 0; // filter
  ihdr[12] = 0; // interlace
  chunks.push(makeChunk("IHDR", ihdr));

  // IDAT
  chunks.push(makeChunk("IDAT", compressed));

  // IEND
  chunks.push(makeChunk("IEND", Buffer.alloc(0)));

  return Buffer.concat(chunks);
}

function makeChunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeB = Buffer.from(type, "ascii");
  const crc = crc32(Buffer.concat([typeB, data]));
  const crcB = Buffer.alloc(4);
  crcB.writeUInt32BE(crc >>> 0, 0);
  return Buffer.concat([len, typeB, data, crcB]);
}

// Standard CRC-32 table
const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = (c & 1) ? (0xedb88320 ^ (c >>> 1)) : (c >>> 1);
    t[n] = c;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff);
}

// Generate all four tray icons
for (const [name, { bg, dot }] of Object.entries(STATES)) {
  const png = makePng(bg, dot);
  const path = join(ICONS_DIR, `${name}.png`);
  const ws = createWriteStream(path);
  ws.write(png);
  ws.end();
  console.log(`  wrote ${path} (${png.length} bytes)`);
}
