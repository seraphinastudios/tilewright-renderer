// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// Browser half of the WebGL2 context-loss test: a lost context ends every
// call and every readback with a typed error instead of a hang or a trap.
// test/smoke.test.mjs asserts on `window.lossResult`.

import { TilewrightError, TilewrightRenderer } from '../dist/index.js';

async function codeOf(action) {
  try {
    await action();
    return 'none';
  } catch (error) {
    return error instanceof TilewrightError ? error.code : `untyped: ${error}`;
  }
}

/** Settles with the code, or `pending` if the promise has not settled in time. */
function settled(promise, millis) {
  return Promise.race([
    codeOf(() => promise),
    new Promise((resolve) => setTimeout(() => resolve('pending'), millis)),
  ]);
}

function lose(canvas) {
  canvas.getContext('webgl2').getExtension('WEBGL_lose_context').loseContext();
}

async function renderer(id) {
  const canvas = document.getElementById(id);
  const created = await TilewrightRenderer.create(canvas, { backend: 'webgl2' });
  const texture = created.uploadTexture({
    id: 1,
    width: 2,
    height: 2,
    rgba: new Uint8Array(16).fill(255),
  });
  created.submit(created.scene().sprite({ id: 1, texture, x: 0, y: 0, width: 8, height: 8 }));
  return { canvas, renderer: created };
}

async function run() {
  // Lost while idle, after a readback and a presented frame, so every
  // pipeline already exists.
  const idle = await renderer('idle');
  await idle.renderer.readFrame();
  const presented = idle.renderer.render();
  lose(idle.canvas);
  const idleCodes = [
    await codeOf(() => idle.renderer.render()),
    await settled(idle.renderer.readFrame(), 5_000),
  ];
  const idleDestroy = await codeOf(() => idle.renderer.destroy());

  // Lost before the first frame: no surface pipeline exists yet.
  const before = await renderer('before');
  lose(before.canvas);
  const beforeCodes = [
    await codeOf(() => before.renderer.render()),
    await settled(before.renderer.readFrame(), 5_000),
  ];
  before.renderer.destroy();

  // Lost while a readback waits for the GPU.
  const during = await renderer('during');
  const waiting = during.renderer.readFrame();
  await new Promise((resolve) => setTimeout(resolve, 0));
  lose(during.canvas);
  const duringCodes = [
    await settled(waiting, 5_000),
    await codeOf(() => during.renderer.render()),
  ];
  during.renderer.destroy();

  // Destroyed while a readback waits on a lost context.
  const destroyed = await renderer('destroyed');
  const abandoned = destroyed.renderer.readFrame();
  lose(destroyed.canvas);
  destroyed.renderer.destroy();
  const destroyedCode = await settled(abandoned, 5_000);

  // Another renderer keeps working.
  const other = await renderer('other');
  const frame = await other.renderer.readFrame();
  const otherPresented = other.renderer.render();
  other.renderer.destroy();

  return {
    presented,
    idleCodes,
    idleDestroy,
    beforeCodes,
    duringCodes,
    destroyedCode,
    other: { width: frame.width, presented: otherPresented, white: frame.rgba[0] },
  };
}

window.lossResult = run().catch((error) => ({ error: String(error), stack: error?.stack }));
