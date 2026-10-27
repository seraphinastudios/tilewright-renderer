// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// Browser half of the trap test: the full bundle, built with test hooks,
// panics in `submit`, inside a readback, or inside `create`. Either way the
// call fails with `internal` instead of hanging, every renderer of that
// bundle stops, and a renderer of the webgpu bundle keeps working.
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

async function renderer(canvas, backend) {
  const created = await TilewrightRenderer.create(canvas, { backend });
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
  const first = await renderer(document.getElementById('first'), 'webgl2');
  const second = await renderer(document.getElementById('second'), 'webgl2');
  const other = await renderer(new OffscreenCanvas(8, 8), 'webgpu');
  if (typeof bindings.panicForTest !== 'function') {
    return { hooks: false };
  }
  bindings.panicForTest(point);
  let message = '';
  const remember = (promise) =>
    promise.catch((error) => {
      message = error.message;
      throw error;
    });
  let trapped;
  if (point === 'submit') {
    trapped = await codeOf(() => first.renderer.submit(first.scene));
  } else if (point === 'readback') {
    trapped = await settled(remember(first.renderer.readFrame()), 5_000);
  } else {
    trapped = await settled(
      remember(TilewrightRenderer.create(new OffscreenCanvas(8, 8), { backend: 'webgl2' })),
      5_000,
    );
  }
  if (message === '') {
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
  const otherBundle = await codeOf(() => other.renderer.readFrame());
  const destroyed = await codeOf(() => {
    first.renderer.destroy();
    second.renderer.destroy();
    other.renderer.destroy();
  });
  return { hooks: true, trapped, message, after, otherBundle, destroyed };
}

window.trapResult = run().catch((error) => ({ error: String(error), stack: error?.stack }));
