// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// Headless Chromium smoke tests: the corpus atlas scene, built with the
// TypeScript API, rendered with WebGPU and with WebGL2, against the CPU
// reference. Set TILEWRIGHT_REQUIRE_WEBGPU=1 to fail rather than skip when
// the browser offers no WebGPU adapter.

import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { after, before, test } from 'node:test';
import { launch } from './browser.mjs';
import { serve } from './server.mjs';

const manifest = JSON.parse(
  await readFile(new URL('../../../corpus/manifest.json', import.meta.url), 'utf8'),
);
const corpus = manifest.cases.find((entry) => entry.name === 'atlas-layered-characters');

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

async function smoke(backend) {
  const page = await browser.newPage();
  const messages = [];
  page.on('console', (message) => messages.push(`${message.type()}: ${message.text()}`));
  page.on('pageerror', (error) => messages.push(`pageerror: ${error.message}`));
  try {
    await page.goto(`${site.base}/test/smoke.html?backend=${backend}`);
    const result = await page.evaluate(() => window.smokeResult);
    return { result, messages };
  } finally {
    await page.close();
  }
}

for (const backend of ['webgl2', 'webgpu']) {
  test(`${backend} renders the atlas scene like the CPU reference`, async (context) => {
    const { result, messages } = await smoke(backend);
    if (result.unavailable) {
      if (backend === 'webgpu' && process.env.TILEWRIGHT_REQUIRE_WEBGPU !== '1') {
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
    assert.equal(result.invalidScene, 'invalid-scene');
    assert.equal(result.conflict, 'invalid-texture');
    assert.equal(result.stale, 'unknown-texture');
    assert.deepEqual(result.busyCodes, ['busy', 'busy', 'busy']);
    assert.deepEqual(result.destroyedCodes, ['destroyed', 'destroyed', 'destroyed']);
    assert.equal(result.wrongSize, 'invalid-target');
    assert.equal(result.forgedRecords, 'invalid-scene');
    assert.deepEqual(result.oversized, [
      'invalid-texture',
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
