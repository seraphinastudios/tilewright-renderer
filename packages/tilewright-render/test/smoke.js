// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// Browser half of the smoke test. Renders the corpus atlas scene on the
// backend named in the query string and reports the comparison with the CPU
// reference as `window.smokeResult`; test/smoke.test.mjs asserts on it.

import { SceneBuilder, TilewrightError, TilewrightRenderer } from '../dist/index.js';
import { ATLAS_ID, SIDE, atlasPixels, atlasScene } from './fixture.js';

const WIDTH = 112;
const HEIGHT = 64;

async function sha256(array) {
  const bytes = new Uint8Array(array.buffer, array.byteOffset, array.byteLength);
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function maximumDifference(left, right) {
  let maximum = 0;
  for (let index = 0; index < left.length; index += 1) {
    maximum = Math.max(maximum, Math.abs(left[index] - right[index]));
  }
  return maximum;
}

function equal(left, right) {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

/** Reads the canvas within the task that rendered it, before compositing. */
function canvasPixels(canvas) {
  const copy = new OffscreenCanvas(canvas.width, canvas.height);
  const context = copy.getContext('2d', { willReadFrequently: true });
  context.drawImage(canvas, 0, 0);
  return context.getImageData(0, 0, canvas.width, canvas.height).data;
}

/** The largest colour difference; the canvas is opaque, so alpha is skipped. */
function colourDifference(screen, rgba) {
  if (screen.length !== rgba.length) {
    return Infinity;
  }
  let maximum = 0;
  for (let index = 0; index < screen.length; index += 4) {
    for (let channel = 0; channel < 3; channel += 1) {
      maximum = Math.max(maximum, Math.abs(screen[index + channel] - rgba[index + channel]));
    }
  }
  return maximum;
}

async function codeOf(action) {
  try {
    await action();
    return 'none';
  } catch (error) {
    return error instanceof TilewrightError ? error.code : `untyped: ${error}`;
  }
}

function scene(renderer, texture) {
  return atlasScene(
    renderer.scene({ clearColor: [0.03, 0.02, 0.05, 1], revision: 4, clockMillis: 375 }),
    texture,
  );
}

async function run() {
  const backend = new URLSearchParams(location.search).get('backend');
  const canvas = document.getElementById('target');
  let renderer;
  try {
    renderer = await TilewrightRenderer.create(canvas, { backend });
  } catch (error) {
    return { backend, unavailable: error.code === 'backend-unavailable', error: String(error) };
  }
  const texture = renderer.uploadTexture({
    id: ATLAS_ID,
    width: SIDE,
    height: SIDE,
    rgba: atlasPixels(),
  });
  renderer.submit(scene(renderer, texture));
  const frame = await renderer.readFrame();
  const reference = renderer.renderReference();
  const presented = renderer.render();
  const screen = canvasPixels(canvas);
  const metrics = renderer.metrics();
  const presentedDifference = colourDifference(screen, reference.rgba);

  // Typed failures.
  const unknown = new SceneBuilder({ width: WIDTH, height: HEIGHT }).sprite({
    id: 1,
    texture: { id: 999, revision: 1 },
    x: 0,
    y: 0,
    width: 4,
    height: 4,
  });
  const unknownTexture = await codeOf(() => renderer.submit(unknown));
  const outside = new SceneBuilder({ width: WIDTH, height: HEIGHT }).sprite({
    id: 1,
    texture,
    frame: { x: 60, y: 60, width: 8, height: 8 },
    x: 0,
    y: 0,
    width: 4,
    height: 4,
  });
  renderer.submit(outside);
  const frameOutside = await codeOf(() => renderer.readFrame());
  const invalidScene = await codeOf(() =>
    renderer.submit(
      new SceneBuilder({ width: WIDTH, height: HEIGHT }).quad({
        id: 1,
        texture,
        positions: [
          [0, 0],
          [Number.NaN, 0],
          [4, 4],
          [0, 4],
        ],
      }),
    ),
  );

  // Resizing keeps the device, textures and caches.
  renderer.resize(56, 32);
  const resized = new SceneBuilder({ width: 56, height: 32, revision: 5 }).sprite({
    id: 7,
    texture,
    frame: { x: 0, y: 48, width: 16, height: 16 },
    x: 8,
    y: 8,
    width: 32,
    height: 16,
  });
  renderer.submit(resized);
  const resizedFrame = await renderer.readFrame();
  const resizedReference = renderer.renderReference();
  const resizedPresented = renderer.render();
  const resizedScreen = canvasPixels(canvas);
  renderer.destroy();

  return {
    backend: renderer.backend,
    bundle: renderer.bundle,
    adapter: renderer.adapter,
    rgbaDifference: maximumDifference(frame.rgba, reference.rgba),
    semanticEqual: equal(frame.semanticIds, reference.semanticIds),
    depthEqual: equal(frame.depth, reference.depth),
    coverageEqual: equal(frame.coverage, reference.coverage),
    pickedObjects: new Set(frame.semanticIds).size,
    presented,
    presentedDifference,
    referenceDigests: {
      rgba8: await sha256(reference.rgba),
      semantic_u32le: await sha256(reference.semanticIds),
      depth_i16le: await sha256(reference.depth),
      coverage_u8: await sha256(reference.coverage),
    },
    metrics,
    unknownTexture,
    frameOutside,
    invalidScene,
    resized: {
      presented: resizedPresented,
      canvas: [canvas.width, canvas.height],
      rgbaDifference: maximumDifference(resizedFrame.rgba, resizedReference.rgba),
      semanticEqual: equal(resizedFrame.semanticIds, resizedReference.semanticIds),
      presentedDifference: colourDifference(resizedScreen, resizedReference.rgba),
    },
  };
}

window.smokeResult = run().catch((error) => ({ error: String(error), stack: error?.stack }));
