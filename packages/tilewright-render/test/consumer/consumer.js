// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// A consumer of the installed release tarball, as a page would use it: it
// imports the package by name and draws one sprite on the backend named in
// the query string, then compares the GPU frame with the CPU reference.
// test/tarball.mjs installs the tarball, serves this page and reads
// `window.consumerResult`.

import { SceneBuilder, TilewrightRenderer } from '@seraphinastudios/tilewright-render';

const SIDE = 16;
const SPRITE_ID = 7;

async function run() {
  const backend = new URLSearchParams(location.search).get('backend');
  const bundle = backend === 'webgpu' ? 'webgpu' : 'full';
  let renderer;
  try {
    renderer = await TilewrightRenderer.create(document.getElementById('target'), { backend, bundle });
  } catch (error) {
    return { unavailable: error.code === 'backend-unavailable', error: String(error) };
  }
  try {
    // A 2 × 2 texture of four opaque colours, drawn over the middle.
    const rgba = new Uint8Array([255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]);
    const texture = renderer.uploadTexture({ id: 1, width: 2, height: 2, rgba });
    const scene = new SceneBuilder({ width: SIDE, height: SIDE, clearColor: [0, 0, 0, 1], revision: 1 })
      .sprite({ id: SPRITE_ID, texture, x: 4, y: 4, width: 8, height: 8 });
    renderer.submit(scene);
    const frame = await renderer.readFrame();
    const reference = renderer.renderReference();
    let rgbaDifference = 0;
    for (let index = 0; index < frame.rgba.length; index += 1) {
      rgbaDifference = Math.max(rgbaDifference, Math.abs(frame.rgba[index] - reference.rgba[index]));
    }
    const covered = frame.semanticIds.filter((id) => id === SPRITE_ID).length;
    const semanticEqual = frame.semanticIds.every((id, index) => id === reference.semanticIds[index]);
    return { backend: renderer.backend, bundle: renderer.bundle, rgbaDifference, covered, semanticEqual };
  } finally {
    renderer.destroy();
  }
}

window.consumerResult = run().catch((error) => ({ error: String(error), stack: error?.stack }));
