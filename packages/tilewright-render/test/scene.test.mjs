// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// SceneBuilder and SpriteSheet run without WebAssembly, so Node tests them
// directly.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  MAXIMUM_QUADS,
  QUAD_RECORD_BYTES,
  SceneBuilder,
  SpriteSheet,
  TilewrightError,
} from '../dist/index.js';

const texture = { id: 1, revision: 1, digest: '0f'.repeat(32) };
const sprite = (id) => ({ id, texture, x: 0, y: 0, width: 1, height: 1 });
const code = (action) => {
  try {
    action();
    return 'none';
  } catch (error) {
    return error instanceof TilewrightError ? error.code : String(error);
  }
};

test('scenes stop at the renderer quad limit before allocating more', () => {
  assert.equal(code(() => new SceneBuilder({ width: 4, height: 4 }, MAXIMUM_QUADS + 1)), 'invalid-argument');
  const scene = new SceneBuilder({ width: 4, height: 4 }, 1);
  for (let id = 1; id <= MAXIMUM_QUADS; id += 1) {
    scene.sprite(sprite(id));
  }
  assert.equal(scene.length, MAXIMUM_QUADS);
  assert.equal(scene.records().buffer.byteLength, MAXIMUM_QUADS * QUAD_RECORD_BYTES);
  assert.equal(code(() => scene.sprite(sprite(1))), 'limit-exceeded');
  assert.equal(scene.length, MAXIMUM_QUADS);
  scene.reset();
  assert.equal(scene.length, 0);
});

test('scene options are validated copies that cannot change', () => {
  const clear = [0.1, 0.2, 0.3, 1];
  const scene = new SceneBuilder({ width: 4, height: 4, clearColor: clear });
  clear[0] = 1;
  assert.equal(scene.options.clearColor[0], 0.1);
  assert.throws(() => {
    scene.options.width = 32.5;
  }, TypeError);
  assert.equal(scene.options.width, 4);
  assert.equal(code(() => scene.reset({ width: 2 ** 32 + 32 })), 'invalid-argument');
  // Exactly four numbers, from an array or a typed array; nothing past them
  // is read.
  assert.deepEqual(
    [...new SceneBuilder({ width: 4, height: 4, clearColor: new Float32Array([0, 0.5, 1, 1]) }).options.clearColor],
    [0, 0.5, 1, 1],
  );
  for (const clearColor of [[0, 0, 0], [0, 0, 0, 1, 1], [0, 0, '0', 1], { length: 100_000_000 }, 'rgba']) {
    assert.equal(code(() => new SceneBuilder({ width: 4, height: 4, clearColor })), 'invalid-argument', String(clearColor));
  }
});

test('records pack every field little-endian', () => {
  const scene = new SceneBuilder({ width: 4, height: 4 }).quad({
    id: 7,
    part: 2,
    depth: -3,
    order: 9,
    texture: { id: 2 ** 40 + 5, revision: 3, digest: '0102'.repeat(16) },
    frame: { x: 1, y: 2, width: 3, height: 4 },
    positions: [
      [0, 0],
      [1, 0],
      [1, 1],
      [0, 1],
    ],
    sampling: 'linear',
    layer: 'effect',
    glow: { color: [1, 0, 0, 1], radius: [0.1, 0.2] },
  });
  const words = new Uint32Array(scene.records().slice().buffer);
  assert.equal(words.length, QUAD_RECORD_BYTES / 4);
  assert.equal(words[0], 7);
  assert.equal(words[1], (2 | (0xfffd << 16)) >>> 0);
  assert.equal(words[2], 9);
  // Not pickable (glow), linear, glow and frame flags; the effect layer.
  assert.equal(words[3], (1 << 1) | (1 << 2) | (1 << 3) | (1 << 8));
  assert.deepEqual([...words.slice(4, 8)], [5, 256, 3, 0]);
  assert.deepEqual([...words.slice(8, 12)], [1, 2, 3, 4]);
  const bytes = scene.records();
  assert.deepEqual([...bytes.slice(156, 160)], [1, 2, 1, 2]);
});

test('integer fields are checked rather than truncated', () => {
  const scene = new SceneBuilder({ width: 4, height: 4 });
  assert.equal(code(() => scene.sprite({ ...sprite(1.5) })), 'invalid-argument');
  assert.equal(code(() => scene.sprite({ ...sprite(1), part: 70_000 })), 'invalid-argument');
  assert.equal(code(() => scene.sprite({ ...sprite(1), texture: { ...texture, id: 0 } })), 'invalid-argument');
  assert.equal(code(() => scene.sprite({ ...sprite(1), texture: { ...texture, digest: 'xyz' } })), 'invalid-argument');
  assert.equal(code(() => scene.sprite({ ...sprite(1), width: 0 })), 'invalid-argument');
  assert.equal(scene.length, 0);
});

test('a short colour or point never keeps an earlier quad\'s values', () => {
  const scene = new SceneBuilder({ width: 4, height: 4 }, 1);
  scene.sprite({ ...sprite(1), color: [1, 1, 1, 0.25] });
  scene.reset();
  for (const quad of [
    { ...sprite(1), color: [1, 0, 0] },
    { ...sprite(1), color: [1, 0, 0, 1, 1] },
    { ...sprite(1), color: [1, 0, 0, '1'] },
    { ...sprite(1), color: { length: 100_000_000 } },
    { ...sprite(1), grayscale: '0.5' },
    { id: 1, texture, positions: [[0, 0], [1, 0], [1, 1]] },
    { id: 1, texture, positions: [[0, 0], [1, 0], [1, 1], [0]] },
    { id: 1, texture, positions: [[0, 0], [1, 0], [1, 1], [0, 1]], uvs: [[0, 0]] },
    {
      id: 1,
      texture,
      positions: [[0, 0], [1, 0], [1, 1], [0, 1]],
      glow: { color: new Array(1_000).fill(1), radius: [0.1, 0.1] },
    },
    { id: 1, texture, positions: [[0, 0], [1, 0], [1, 1], [0, 1]], glow: { color: [1, 1, 1, 1], radius: [0.1] } },
  ]) {
    const add = () => (quad.positions ? scene.quad(quad) : scene.sprite(quad));
    assert.equal(code(add), 'invalid-argument', JSON.stringify(quad).slice(0, 80));
  }
  assert.equal(scene.length, 0);
  // Typed arrays are accepted, and sprites add their edges in f32: in f64,
  // 0.3 + 0.6 and 0.4 + 1.3 round to other f32 values.
  scene.sprite({ ...sprite(1), color: new Float32Array([1, 0, 0, 1]), x: 0.3, width: 0.6, y: 0.4, height: 1.3 });
  const floats = new Float32Array(scene.records().slice().buffer);
  assert.deepEqual([...floats.slice(28, 32)], [1, 0, 0, 1]);
  const right = Math.fround(Math.fround(0.3) + Math.fround(0.6));
  const bottom = Math.fround(Math.fround(0.4) + Math.fround(1.3));
  assert.notEqual(right, Math.fround(0.3 + 0.6));
  assert.notEqual(bottom, Math.fround(0.4 + 1.3));
  assert.deepEqual([...floats.slice(12, 20)], [Math.fround(0.3), Math.fround(0.4), right, Math.fround(0.4), right, bottom, Math.fround(0.3), bottom]);
});

test('sprite sheets number frames row by row', () => {
  const sheet = SpriteSheet.covering({ width: 832, height: 1344 }, 64, 64);
  assert.equal(sheet.frameCount, 273);
  assert.deepEqual(sheet.frame(14), { x: 64, y: 64, width: 64, height: 64 });
  assert.deepEqual(sheet.frame(272), sheet.frameAt(12, 20));
  assert.equal(code(() => sheet.frame(273)), 'invalid-argument');
  // Coordinates past 32 bits are refused, and so is a count past them.
  const huge = SpriteSheet.grid(2, 2, 0xffff_ffff, 2);
  assert.equal(huge.frameCount, undefined);
  assert.equal(code(() => huge.frameAt(0x8000_0000, 0)), 'invalid-argument');
  assert.deepEqual(huge.frame(0), { x: 0, y: 0, width: 2, height: 2 });
  const spaced = new SpriteSheet({ frameWidth: 8, frameHeight: 10, columns: 4, rows: 2, origin: [2, 3], spacing: [1, 2] });
  assert.deepEqual(spaced.frame(5), { x: 11, y: 15, width: 8, height: 10 });
});
