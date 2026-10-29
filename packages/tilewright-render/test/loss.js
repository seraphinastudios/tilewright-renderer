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

  // Lost while a readback waits for the GPU: the readback's first poll runs
  // in a microtask queued before this function resumes.
  const during = await renderer('during');
  const waiting = during.renderer.readFrame();
  await Promise.resolve();
  lose(during.canvas);
  const duringCodes = [
    await settled(waiting, 5_000),
    await codeOf(() => during.renderer.render()),
  ];
  during.renderer.destroy();

  // Destroyed while a readback waits on a loss the renderer cannot see, as
  // if the context never reported it: only destroying ends the wait.
  const destroyed = await renderer('destroyed');
  const isContextLost = WebGL2RenderingContext.prototype.isContextLost;
  WebGL2RenderingContext.prototype.isContextLost = () => false;
  let destroyedCodes;
  try {
    const abandoned = destroyed.renderer.readFrame();
    await Promise.resolve();
    lose(destroyed.canvas);
    const beforeDestroy = await settled(abandoned, 300);
    destroyed.renderer.destroy();
    destroyedCodes = [beforeDestroy, await settled(abandoned, 5_000)];
  } finally {
    WebGL2RenderingContext.prototype.isContextLost = isContextLost;
  }

  // finish() waiting for the GPU ends the same ways: with device-lost when
  // the context is lost, and, when the loss is hidden, only on destroy.
  const finishing = await renderer('finishing');
  finishing.renderer.render();
  const lostFinish = finishing.renderer.finish();
  await Promise.resolve();
  lose(finishing.canvas);
  const finishCodes = [await settled(lostFinish, 5_000)];
  finishing.renderer.destroy();
  const hidden = await renderer('hidden');
  WebGL2RenderingContext.prototype.isContextLost = () => false;
  try {
    hidden.renderer.render();
    const abandoned = hidden.renderer.finish();
    await Promise.resolve();
    lose(hidden.canvas);
    finishCodes.push(await settled(abandoned, 300));
    hidden.renderer.destroy();
    finishCodes.push(await settled(abandoned, 5_000));
  } finally {
    WebGL2RenderingContext.prototype.isContextLost = isContextLost;
  }

  // A canvas whose context is lost is refused with a typed error rather
  // than trapping the module.
  const lostCodes = [
    await codeOf(() => TilewrightRenderer.create(idle.canvas, { backend: 'webgl2' })),
    await codeOf(() => TilewrightRenderer.create(during.canvas, { backend: 'webgl2' })),
  ];

  // A destroyed renderer leaves its canvas usable: re-creating on the same
  // canvas, as a re-mounted view does, reuses its context, and another
  // renderer keeps drawing throughout.
  const other = await renderer('other');
  const reused = document.createElement('canvas');
  reused.width = 8;
  reused.height = 8;
  let reusedWhite = 0;
  for (let index = 0; index < 20; index += 1) {
    const again = await TilewrightRenderer.create(reused, { backend: 'webgl2' });
    const texture = again.uploadTexture({
      id: 1,
      width: 1,
      height: 1,
      rgba: new Uint8Array([255, 255, 255, 255]),
    });
    again.submit(again.scene().sprite({ id: 1, texture, x: 0, y: 0, width: 8, height: 8 }));
    reusedWhite += (await again.readFrame()).rgba[0] === 255 ? 1 : 0;
    again.destroy();
  }
  const frame = await other.renderer.readFrame();
  const otherPresented = other.renderer.render();
  other.renderer.destroy();

  return {
    presented,
    idleCodes,
    idleDestroy,
    beforeCodes,
    duringCodes,
    destroyedCodes,
    lostCodes,
    finishCodes,
    reusedWhite,
    other: { width: frame.width, presented: otherPresented, white: frame.rgba[0] },
  };
}

window.lossResult = run().catch((error) => ({ error: String(error), stack: error?.stack }));
