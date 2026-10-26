// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// Browser half of the trap test: the full bundle, built with test hooks,
// panics in `submit` or inside a readback. Either way the call fails with
// `internal` instead of hanging, and every renderer of the page stops.
// test/smoke.test.mjs asserts on `window.trapResult`.

import { TilewrightError, TilewrightRenderer } from '../dist/index.js';
// The module instance the renderers use: the same URL loads it once.
import * as bindings from '../wasm/full/tilewright_render_web.js';

async function codeOf(action) {
  try {
    await action();
    return 'none';
  } catch (error) {
    return error instanceof TilewrightError ? error.code : `untyped: ${error}`;
  }
}

function settled(promise, millis) {
  return Promise.race([
    codeOf(() => promise),
    new Promise((resolve) => setTimeout(() => resolve('pending'), millis)),
  ]);
}

async function renderer(id) {
  const created = await TilewrightRenderer.create(document.getElementById(id), {
    backend: 'webgl2',
  });
  const texture = created.uploadTexture({
    id: 1,
    width: 1,
    height: 1,
    rgba: new Uint8Array([255, 255, 255, 255]),
  });
  const scene = created.scene().sprite({ id: 1, texture, x: 0, y: 0, width: 8, height: 8 });
  created.submit(scene);
  return { renderer: created, scene };
}

async function run() {
  const point = new URLSearchParams(location.search).get('point');
  const first = await renderer('first');
  const second = await renderer('second');
  if (typeof bindings.panicForTest !== 'function') {
    return { hooks: false };
  }
  bindings.panicForTest(point);
  let message = '';
  const trapped =
    point === 'submit'
      ? await codeOf(() => first.renderer.submit(first.scene))
      : await settled(
          first.renderer.readFrame().catch((error) => {
            message = error.message;
            throw error;
          }),
          5_000,
        );
  if (point === 'submit') {
    try {
      first.renderer.render();
    } catch (error) {
      message = error.message;
    }
  }
  const after = [
    await codeOf(() => first.renderer.render()),
    await codeOf(() => second.renderer.readFrame()),
    await codeOf(() => TilewrightRenderer.create(new OffscreenCanvas(8, 8), { backend: 'webgl2' })),
  ];
  const destroyed = await codeOf(() => {
    first.renderer.destroy();
    second.renderer.destroy();
  });
  return { hooks: true, trapped, message, after, destroyed };
}

window.trapResult = run().catch((error) => ({ error: String(error), stack: error?.stack }));
