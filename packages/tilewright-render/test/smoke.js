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
    texture: { id: 999, revision: 1, digest: 'ab'.repeat(32) },
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

  // A stored texture never changes, and a stale identity is refused.
  const conflict = await codeOf(() =>
    renderer.uploadTexture({ id: ATLAS_ID, width: 1, height: 1, rgba: new Uint8Array(4) }),
  );
  const staleScene = new SceneBuilder({ width: WIDTH, height: HEIGHT }).sprite({
    id: 1,
    texture: { ...texture, digest: 'cd'.repeat(32) },
    x: 0,
    y: 0,
    width: 4,
    height: 4,
  });
  const stale = await codeOf(() => renderer.submit(staleScene));

  // A scene of another size, or malformed records, never replace the
  // current scene.
  const wrongSize = await codeOf(() =>
    renderer.submit(new SceneBuilder({ width: WIDTH + 1, height: HEIGHT })),
  );
  const forged = new SceneBuilder({ width: WIDTH, height: HEIGHT });
  forged.records = () => new Uint8Array(10);
  const forgedRecords = await codeOf(() => renderer.submit(forged));
  class ForgedOptions extends SceneBuilder {
    get options() {
      return { ...super.options, clearColor: { length: 100_000_000 } };
    }
  }
  const forgedClear = await codeOf(() =>
    renderer.submit(new ForgedOptions({ width: WIDTH, height: HEIGHT })),
  );

  // Removal needs the whole identity: a stale digest removes nothing, so
  // the atlas still draws below.
  const extra = renderer.uploadTexture({ id: 61, width: 1, height: 1, rgba: new Uint8Array(4) });
  const removed = [
    renderer.removeTexture({ ...texture, digest: 'cd'.repeat(32) }),
    renderer.removeTexture(extra),
    renderer.removeTexture(extra),
  ];

  // A readback makes the renderer busy from the call until it settles.
  renderer.submit(scene(renderer, texture));
  const pending = renderer.readFrame();
  const busyCodes = [
    await codeOf(() => renderer.submit(scene(renderer, texture))),
    await codeOf(() => renderer.resize(40, 40)),
    await codeOf(() =>
      renderer.uploadTexture({ id: 60, width: 1, height: 1, rgba: new Uint8Array(4) }),
    ),
  ];
  await pending;

  // Oversized textures are refused before their pixels are read or copied.
  const tooWide = await codeOf(() =>
    renderer.uploadTexture({ id: 50, width: 8_193, height: 1, rgba: new Uint8Array(4) }),
  );
  const tooLarge = await codeOf(() =>
    renderer.uploadTexture({ id: 51, width: 4_096, height: 4_096, rgba: new Uint8Array(4) }),
  );
  const imageTooLarge = await codeOf(() =>
    renderer.uploadImage(52, { width: 4_096, height: 4_096 }),
  );
  const shortPixels = await codeOf(() =>
    renderer.uploadTexture({ id: 53, width: 2, height: 2, rgba: new Uint8Array(15) }),
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

  // Destroying during a readback rejects it; later calls fail too. A second
  // renderer without a scene is destroyed before its readback starts.
  const inFlight = renderer.readFrame();
  renderer.destroy();
  const spare = await TilewrightRenderer.create(new OffscreenCanvas(8, 8), { backend });
  const early = spare.readFrame();
  spare.destroy();
  const destroyedCodes = [
    await codeOf(() => inFlight),
    await codeOf(() => renderer.readFrame()),
    await codeOf(() => early),
  ];

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
    oversized: [tooWide, tooLarge, imageTooLarge, shortPixels],
    conflict,
    stale,
    wrongSize,
    forgedRecords,
    forgedClear,
    removed,
    busyCodes,
    destroyedCodes,
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
