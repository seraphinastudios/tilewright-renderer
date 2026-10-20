// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// The crowd scene from tilewright-render-testkit (src/crowd.rs), ported to
// the TypeScript API: a ground of atlas tiles under walking three-layer
// characters drawn from 13 × 21 sheets of 64 × 64 frames. The same frame
// number gives the same quads as the Rust scene. Every texel is generated
// from constants.

import { SpriteSheet } from '../dist/index.js';
import { frameTexel } from '../test/fixture.js';

export const FRAME = 64;
export const SHEET = SpriteSheet.grid(FRAME, FRAME, 13, 21);
export const TILE = 32;
export const TILES_ID = 301;
export const LAYER_IDS = [302, 303, 304];
export const ATLAS_ID = 305;
const WALK_ROW = 8;
const WALK_FRAMES = 9;
const SHEET_WIDTH = 13 * FRAME;
const SHEET_HEIGHT = 21 * FRAME;
const MASK = (1n << 64n) - 1n;

/** SplitMix64's finaliser, as testkit's `mix`. */
export function mix(index) {
  let value = (BigInt(index) + 0x9e3779b97f4a7c15n) & MASK;
  value = ((value ^ (value >> 30n)) * 0xbf58476d1ce4e5b9n) & MASK;
  value = ((value ^ (value >> 27n)) * 0x94d049bb133111ebn) & MASK;
  return value ^ (value >> 31n);
}

function tileTexel(tile, x, y) {
  return frameTexel(tile, 3, Math.floor(x / 2), Math.floor(y / 2));
}

function layerTexel(layer, row, column, x, y) {
  const [red, green, blue, alpha] = frameTexel(
    column % 4,
    layer,
    Math.floor(x / 4),
    Math.floor(y / 4),
  );
  const shade = (channel) => Math.min(Math.floor((channel * (200 + row * 2)) / 242), 255);
  return [shade(red), shade(green), shade(blue), alpha];
}

function layerSheet(layer, pixels, offset) {
  for (let y = 0; y < SHEET_HEIGHT; y += 1) {
    for (let x = 0; x < SHEET_WIDTH; x += 1) {
      const texel = layerTexel(
        layer,
        Math.floor(y / FRAME),
        Math.floor(x / FRAME),
        x % FRAME,
        y % FRAME,
      );
      pixels.set(texel, offset + (y * SHEET_WIDTH + x) * 4);
    }
  }
}

/** The crowd's textures for `sheets` ('atlas' or 'layers'), as uploads. */
export function crowdTextures(sheets) {
  const tiles = new Uint8Array(TILE * 4 * TILE * 4);
  for (let y = 0; y < TILE; y += 1) {
    for (let x = 0; x < TILE * 4; x += 1) {
      tiles.set(tileTexel(Math.floor(x / TILE), x % TILE, y), (y * TILE * 4 + x) * 4);
    }
  }
  const textures = [{ id: TILES_ID, width: TILE * 4, height: TILE, rgba: tiles }];
  if (sheets === 'layers') {
    for (let layer = 0; layer < 3; layer += 1) {
      const pixels = new Uint8Array(SHEET_WIDTH * SHEET_HEIGHT * 4);
      layerSheet(layer, pixels, 0);
      textures.push({ id: LAYER_IDS[layer], width: SHEET_WIDTH, height: SHEET_HEIGHT, rgba: pixels });
    }
  } else {
    const pixels = new Uint8Array(SHEET_WIDTH * SHEET_HEIGHT * 3 * 4);
    for (let layer = 0; layer < 3; layer += 1) {
      layerSheet(layer, pixels, layer * SHEET_WIDTH * SHEET_HEIGHT * 4);
    }
    textures.push({ id: ATLAS_ID, width: SHEET_WIDTH, height: SHEET_HEIGHT * 3, rgba: pixels });
  }
  return textures;
}

/** Per-character constants, computed once: direction, start and phase. */
export function crowdWalkers(characters) {
  const walkers = [];
  for (let character = 0; character < characters; character += 1) {
    const seed = mix(BigInt(character) + 0x1000_0000n);
    walkers.push({
      direction: Number(seed % 4n),
      startX: Number((seed >> 8n) & 0xffffn),
      startY: Number((seed >> 24n) & 0xffffn),
      phase: Number((seed >> 40n) % BigInt(WALK_FRAMES)),
    });
  }
  return walkers;
}

const STEPS = [
  [0, -1],
  [-1, 0],
  [0, 1],
  [1, 0],
];
const mod = (value, divisor) => ((value % divisor) + divisor) % divisor;

/**
 * Fills `scene` with the crowd at `frame`. `textures` maps each texture ID
 * to its handle; `walkers` comes from `crowdWalkers`.
 */
export function crowdScene(scene, { width, height, sheets }, walkers, textures, frame) {
  const columns = Math.ceil(width / TILE);
  const rows = Math.ceil(height / TILE);
  const tileTexture = textures.get(TILES_ID);
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      const index = row * columns + column;
      const tile = Number(mix(index) % 4n);
      scene.sprite({
        id: 1 + index,
        texture: tileTexture,
        frame: { x: tile * TILE, y: 0, width: TILE, height: TILE },
        x: column * TILE,
        y: row * TILE,
        width: TILE,
        height: TILE,
        depth: -1,
        order: index,
        pickable: false,
      });
    }
  }
  const spanX = width + FRAME;
  const spanY = height + FRAME;
  const travel = frame % 1_000_000;
  const first = columns * rows + 1;
  const atlas = textures.get(ATLAS_ID);
  for (let character = 0; character < walkers.length; character += 1) {
    const walker = walkers[character];
    const [stepX, stepY] = STEPS[walker.direction];
    const x = mod(walker.startX + stepX * travel, spanX) - FRAME;
    const y = mod(walker.startY + stepY * travel, spanY) - FRAME;
    const column = (Math.floor(frame / 4) + walker.phase) % WALK_FRAMES;
    const row = WALK_ROW + walker.direction;
    const order = (y + FRAME) * 4;
    for (let layer = 0; layer < 3; layer += 1) {
      const frameRect = SHEET.frameAt(column, row);
      const texture = sheets === 'layers' ? textures.get(LAYER_IDS[layer]) : atlas;
      scene.sprite({
        id: first + character,
        part: layer,
        texture,
        frame:
          sheets === 'layers'
            ? frameRect
            : { ...frameRect, y: frameRect.y + layer * SHEET_HEIGHT },
        x,
        y,
        width: FRAME,
        height: FRAME,
        order: order + layer,
      });
    }
  }
  return scene;
}
