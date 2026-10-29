// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// Headless Chromium smoke tests: the corpus atlas scene, built with the
// TypeScript API, rendered with WebGPU and with WebGL2, against the CPU
// reference; lost WebGL2 contexts; and traps. Both backends are required:
// set TILEWRIGHT_REQUIRE_WEBGPU=0 to skip, rather than fail, WebGPU where the
// browser offers no adapter. The trap tests need a full bundle built with
// TILEWRIGHT_WEB_TEST_HOOKS=1, as tools/validate-web.sh builds it; with that
// variable set they fail rather than skip without the hooks.

import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { after, before, test } from 'node:test';
import { launch } from './browser.mjs';
import { serve } from './server.mjs';

const manifest = JSON.parse(
  await readFile(new URL('../../../corpus/manifest.json', import.meta.url), 'utf8'),
);
const corpus = manifest.cases.find((entry) => entry.name === 'atlas-layered-characters');
// A hung page fails its test with a diagnostic instead of stalling CI.
const TIMEOUT = { timeout: 120_000 };

let browser;
let site;

before(async () => {
  site = await serve();
  browser = await launch();
});

after(async () => {
  await browser?.close();
  site?.server.close();
});

/** Loads a test page and returns the result it publishes on `window`. */
async function visit(path, name) {
  const page = await browser.newPage();
  const messages = [];
  page.on('console', (message) => messages.push(`${message.type()}: ${message.text()}`));
  page.on('pageerror', (error) => messages.push(`pageerror: ${error.message}`));
  try {
    await page.goto(`${site.base}/test/${path}`);
    const result = await page.evaluate((key) => window[key], name);
    return { result, messages };
  } finally {
    await page.close();
  }
}

const smoke = (backend) => visit(`smoke.html?backend=${backend}`, 'smokeResult');

for (const backend of ['webgl2', 'webgpu']) {
  test(`${backend} renders the atlas scene like the CPU reference`, TIMEOUT, async (context) => {
    const { result, messages } = await smoke(backend);
    if (result.unavailable) {
      if (backend === 'webgpu' && process.env.TILEWRIGHT_REQUIRE_WEBGPU === '0') {
        context.skip(`no WebGPU adapter: ${result.error}`);
        return;
      }
    }
    assert.equal(result.error, undefined, `${result.error}\n${result.stack}\n${messages.join('\n')}`);
    context.diagnostic(
      `backend=${result.backend} bundle=${result.bundle} adapter=${JSON.stringify(result.adapter)}`,
    );
    assert.equal(result.backend, backend);
    assert.equal(result.bundle, backend === 'webgpu' ? 'webgpu' : 'full');
    // The TypeScript-built scene is the corpus scene: the reference renders
    // it to the manifest's exact digests.
    assert.deepEqual(result.referenceDigests, corpus.digests);
    assert.ok(result.rgbaDifference <= corpus.maximum_rgba_channel_difference, `RGBA differs by ${result.rgbaDifference}`);
    assert.ok(result.semanticEqual, 'semantic plane differs');
    assert.ok(result.depthEqual, 'depth plane differs');
    assert.ok(result.coverageEqual, 'coverage plane differs');
    assert.ok(result.pickedObjects > 20);
    assert.equal(result.presented, true);
    assert.ok(result.presentedDifference <= 1, `canvas differs by ${result.presentedDifference}`);
    assert.equal(result.metrics.resourceRequests, 0);
    // The metrics describe the present after the readback.
    assert.equal(result.metrics.readbackBytes, 0);
    assert.equal(typeof result.metrics.targetAllocationBytes, 'number');
    assert.equal(result.metrics.vertexCount, corpus.performance.vertex_count);
    assert.equal(result.unknownTexture, 'unknown-texture');
    assert.equal(result.frameOutside, 'resource');
    assert.equal(result.failedRender, 'resource');
    assert.ok(result.failedDifference <= 1, `a failed render changed the canvas by ${result.failedDifference}`);
    assert.deepEqual(result.direct, {
      presented: true,
      canvas: [56, 32],
      reconfigurations: 1,
      presentedDifference: result.direct.presentedDifference,
    });
    assert.ok(result.direct.presentedDifference <= 1, `canvas differs by ${result.direct.presentedDifference}`);
    // Canvases are opaque on both backends, whatever the clear alpha.
    assert.equal(result.transparent.minimumAlpha, 255);
    assert.ok(result.transparent.difference <= 1, `canvas differs by ${result.transparent.difference}`);
    assert.equal(result.invalidScene, 'invalid-scene');
    assert.equal(result.conflict, 'invalid-texture');
    assert.equal(result.stale, 'unknown-texture');
    assert.deepEqual(result.busyCodes, ['busy', 'busy', 'busy']);
    assert.deepEqual(result.finishBusy, ['busy', 'busy', 'busy']);
    assert.equal(result.finished, 'none');
    assert.deepEqual(result.destroyedCodes, [
      'destroyed',
      'destroyed',
      'destroyed',
      'destroyed',
      'destroyed',
    ]);
    assert.equal(result.wrongSize, 'invalid-target');
    assert.equal(result.forgedRecords, 'invalid-scene');
    // The failed submits left the scene with a frame outside its texture.
    assert.equal(result.keptScene, 'resource');
    assert.equal(result.forgedClear, 'invalid-argument');
    assert.deepEqual(result.removed, [false, true, false]);
    assert.deepEqual(result.oversized, [
      'limit-exceeded',
      'limit-exceeded',
      'limit-exceeded',
      'invalid-texture',
    ]);
    assert.deepEqual(result.resized.canvas, [56, 32]);
    assert.equal(result.resized.presented, true);
    assert.ok(result.resized.rgbaDifference <= 1);
    assert.ok(result.resized.semanticEqual);
    assert.ok(result.resized.presentedDifference <= 1);
  });
}

test('a lost WebGL2 context fails calls and readbacks with device-lost', TIMEOUT, async () => {
  const { result, messages } = await visit('loss.html', 'lossResult');
  assert.equal(result.error, undefined, `${result.error}\n${result.stack}\n${messages.join('\n')}`);
  assert.equal(result.presented, true);
  assert.deepEqual(result.idleCodes, ['device-lost', 'device-lost']);
  assert.equal(result.idleDestroy, 'none');
  assert.deepEqual(result.beforeCodes, ['device-lost', 'device-lost']);
  assert.deepEqual(result.duringCodes, ['device-lost', 'device-lost']);
  assert.deepEqual(result.destroyedCodes, ['pending', 'destroyed']);
  assert.deepEqual(result.finishCodes, ['device-lost', 'pending', 'destroyed']);
  assert.deepEqual(result.lostCodes, ['device-lost', 'device-lost']);
  assert.equal(result.reusedWhite, 20);
  assert.deepEqual(result.other, { width: 8, presented: true, white: 255 });
});

for (const point of ['submit', 'readback', 'create']) {
  test(`a trap in ${point} stops its bundle's renderers with internal`, TIMEOUT, async (context) => {
    const { result, messages } = await visit(`trap.html?point=${point}`, 'trapResult');
    assert.equal(result.error, undefined, `${result.error}\n${result.stack}\n${messages.join('\n')}`);
    if (!result.hooks) {
      assert.notEqual(process.env.TILEWRIGHT_WEB_TEST_HOOKS, '1', 'the full bundle has no test hooks');
      context.skip('the full bundle has no test hooks; build it with TILEWRIGHT_WEB_TEST_HOOKS=1');
      return;
    }
    assert.equal(result.trapped, 'internal');
    assert.match(result.message, new RegExp(`test panic at ${point}`));
    assert.deepEqual(result.after, ['internal', 'internal', 'internal']);
    assert.equal(result.destroyed, 'none');
    // The webgpu bundle is another module and keeps working, where the
    // browser offers WebGPU.
    if (result.otherBundle === 'unavailable') {
      assert.equal(process.env.TILEWRIGHT_REQUIRE_WEBGPU, '0', 'no WebGPU adapter');
    } else {
      assert.equal(result.otherBundle, 'none');
    }
  });
}
