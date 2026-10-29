// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// The corpus atlas and its layered-character scene, ported from
// tilewright-render-testkit (src/atlas.rs) to the TypeScript API. Every
// texel is generated from constants; no game or third-party art is used.

import { SpriteSheet } from '../dist/index.js';

export const ATLAS_ID = 201;
export const FRAME = 16;
export const SIDE = FRAME * 4;
export const SHEET = SpriteSheet.grid(FRAME, FRAME, 4, 4);
const TUNICS = [
  [60, 90, 200],
  [200, 60, 80],
  [70, 160, 90],
  [180, 150, 40],
];
const HAIR = [
  [90, 60, 30],
  [30, 30, 35],
  [200, 170, 90],
  [150, 70, 40],
];
const GROUND = [
  [
    [70, 150, 60],
    [60, 130, 55],
  ],
  [
    [210, 190, 120],
    [190, 170, 105],
  ],
  [
    [120, 120, 130],
    [95, 95, 110],
  ],
  [
    [60, 110, 170],
    [50, 95, 150],
  ],
];

export function frameTexel(column, row, x, y) {
  const opaque = ([r, g, b]) => [r, g, b, 255];
  if (row === 0) {
    const phase = [0, 1, 0, -1][column];
    const head = (x - 8) * (x - 8) + (y - 4) * (y - 4) <= 9;
    const torso = x >= 5 && x < 11 && y >= 7 && y < 12;
    const legs =
      y >= 12 &&
      y < 16 &&
      ((x >= 5 + phase && x < 7 + phase) || (x >= 9 - phase && x < 11 - phase));
    return head || torso || legs ? opaque([224, 172, 140]) : [0, 0, 0, 0];
  }
  if (row === 1) {
    if (x >= 4 && x < 12 && y >= 7 && y < 13) {
      const tunic = TUNICS[column];
      return y === 11 ? opaque(tunic.map((channel) => Math.floor(channel / 3))) : opaque(tunic);
    }
    return [0, 0, 0, 0];
  }
  if (row === 2) {
    if (x >= 4 && x < 12 && y >= 0 && y < 4) {
      const [r, g, b] = HAIR[column];
      return [r, g, b, y === 3 ? 128 : 255];
    }
    return [0, 0, 0, 0];
  }
  const [primary, secondary] = GROUND[column];
  if (x === 15 || y === 15) {
    return opaque([250, 250, 250]);
  }
  return opaque((Math.floor(x / 4) + Math.floor(y / 4)) % 2 === 0 ? primary : secondary);
}

/** The atlas as RGBA8 pixels. */
export function atlasPixels() {
  const pixels = new Uint8Array(SIDE * SIDE * 4);
  for (let y = 0; y < SIDE; y += 1) {
    for (let x = 0; x < SIDE; x += 1) {
      const texel = frameTexel(
        Math.floor(x / FRAME),
        Math.floor(y / FRAME),
        x % FRAME,
        y % FRAME,
      );
      pixels.set(texel, (y * SIDE + x) * 4);
    }
  }
  return pixels;
}

const frame = (column, row) => SHEET.frameAt(column, row);
const box = (left, top, right, bottom) => [
  [left, top],
  [right, top],
  [right, bottom],
  [left, bottom],
];

/** Adds the body, tunic and hair layers of one character. */
export function character(scene, texture, id, step, [left, top], scale, order, sampling) {
  const size = FRAME * scale;
  for (let layer = 0; layer < 3; layer += 1) {
    scene.quad({
      id,
      part: layer,
      texture,
      frame: frame(step % 4, layer),
      positions: box(left, top, left + size, top + size),
      order: order + layer,
      sampling,
    });
  }
}

/**
 * Fills a 112 × 64 scene exactly as testkit's `atlas_scene`: the same
 * quads, frames, positions and effects, and the same parity rules (no edge
 * or texel boundary on a pixel centre, linear fractions in eighths, at most
 * one translucent layer per pixel).
 */
export function atlasScene(scene, texture) {
  for (let row = 0; row < 4; row += 1) {
    for (let column = 0; column < 4; column += 1) {
      const index = row * 4 + column;
      scene.quad({
        id: 1 + index,
        texture,
        frame: frame((row + column) % 4, 3),
        positions: box(column * FRAME, row * FRAME, column * FRAME + 16, row * FRAME + 16),
        depth: -1,
        order: index,
      });
    }
  }
  for (let step = 0; step < 4; step += 1) {
    const [origin, scale] =
      step === 3 ? [[40.25, 36.25], 1.5] : [[8 + step * 16, 8 + step * 5], 1];
    character(scene, texture, 101 + step, step, origin, scale, 10 + step * 3, 'nearest');
  }
  scene.quad({
    id: 105,
    texture,
    frame: frame(1, 0),
    positions: box(2.25, 30.25, 34.25, 62.25),
    order: 30,
    sampling: 'linear',
  });
  scene.quad({
    id: 200,
    texture,
    frame: frame(1, 3),
    positions: box(66.25, 1.25, 98.25, 33.25),
    sampling: 'linear',
    layer: 'effect',
  });
  scene.quad({
    id: 201,
    texture,
    frame: frame(3, 3),
    positions: box(99.25, 2.25, 111.25, 14.25),
    uvs: box(-0.25, -0.25, 1.25, 1.25),
    sampling: 'linear',
    layer: 'effect',
    order: 1,
    color: [1, 1, 1, 0.85],
  });
  scene.quad({
    id: 202,
    texture,
    frame: frame(2, 3),
    positions: [
      [77.07, 37.07],
      [90.93, 45.07],
      [82.93, 58.93],
      [69.07, 50.93],
    ],
    layer: 'effect',
    order: 2,
  });
  scene.quad({
    id: 203,
    texture,
    frame: frame(2, 2),
    positions: box(92, 38, 112, 58),
    uvs: box(-0.125, -0.125, 1.125, 1.125),
    glow: { color: [1, 0.85, 0.3, 0.9], radius: [0.2, 0.2] },
    pickable: false,
    order: 3,
  });
  scene.quad({
    id: 204,
    texture,
    frame: frame(2, 2),
    positions: box(94, 40, 110, 56),
    order: 4,
  });
  return scene;
}
