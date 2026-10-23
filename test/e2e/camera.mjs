// lispm-remote: the QR code, as a camera sees it.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

// Chromium's fake camera plays a YUV4MPEG2 file.  This makes one of the
// plain PBM picture `lispm-remote-write-qr' writes: the code, black on
// white, in the middle of a square frame a little larger than it.

/** The Y4M file of one frame showing the PBM picture `pbm` (P1, text). */
export function pbmToY4m(pbm) {
  const tokens = pbm.replace(/#[^\n]*/g, "").split(/\s+/).filter(Boolean);
  if (tokens[0] !== "P1") {
    throw new Error("not a plain PBM picture");
  }
  const width = Number(tokens[1]);
  const height = Number(tokens[2]);
  const bits = tokens.slice(3).join("");
  // The frame: square, even, with a margin of white around the code.
  const side = 2 * Math.ceil((Math.max(width, height) + 80) / 2);
  const left = Math.floor((side - width) / 2);
  const top = Math.floor((side - height) / 2);
  const luma = Buffer.alloc(side * side, 255);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      if (bits[y * width + x] === "1") {
        luma[(top + y) * side + left + x] = 0;
      }
    }
  }
  const chroma = Buffer.alloc((side / 2) * (side / 2), 128);
  return Buffer.concat([
    Buffer.from(`YUV4MPEG2 W${side} H${side} F30:1 Ip A1:1 C420jpeg\nFRAME\n`, "ascii"),
    luma,
    chroma,
    chroma,
  ]);
}
