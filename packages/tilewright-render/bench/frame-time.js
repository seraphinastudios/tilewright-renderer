// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// Browser half of the frame-time harness. Query parameters: backend,
// width, height, characters, sheets ('atlas' or 'layers'), frames, warmup
// and mode. In 'serial' mode each frame builds the crowd scene, submits and
// presents it, then waits for the GPU; the frame time is that whole
// sequence. In 'raf' mode frames follow requestAnimationFrame without
// waiting for the GPU, as a game loop does, and the interval between
// callbacks is what a player sees. The result is `window.benchResult`.

import { TilewrightRenderer } from '../dist/index.js';
import { crowdScene, crowdTextures, crowdWalkers } from './crowd.js';

const query = new URLSearchParams(location.search);
const number = (name, fallback) => Number(query.get(name) ?? fallback);
const options = {
  backend: query.get('backend') ?? 'auto',
  width: number('width', 1920),
  height: number('height', 1080),
  characters: number('characters', 1000),
  sheets: query.get('sheets') ?? 'atlas',
  frames: number('frames', 300),
  warmup: number('warmup', 30),
  mode: query.get('mode') ?? 'serial',
};

/** What the browser reports about its GPU, which wgpu does not expose. */
async function deviceDescription(backend) {
  if (backend === 'webgpu') {
    const adapter = await navigator.gpu?.requestAdapter();
    const info = adapter?.info;
    return info ? [info.vendor, info.architecture, info.device, info.description].filter(Boolean).join(' ') : '';
  }
  const gl = new OffscreenCanvas(1, 1).getContext('webgl2');
  const extension = gl?.getExtension('WEBGL_debug_renderer_info');
  return extension ? gl.getParameter(extension.UNMASKED_RENDERER_WEBGL) : '';
}

function nextFrame() {
  return new Promise((resolve) => requestAnimationFrame(resolve));
}

async function run() {
  const canvas = document.getElementById('target');
  canvas.width = options.width;
  canvas.height = options.height;
  const renderer = await TilewrightRenderer.create(canvas, { backend: options.backend });
  const textures = new Map();
  for (const texture of crowdTextures(options.sheets)) {
    textures.set(texture.id, renderer.uploadTexture(texture));
  }
  const walkers = crowdWalkers(options.characters);
  const scene = renderer.scene({ clearColor: [0.05, 0.05, 0.08, 1] });
  const samples = { frame: [], build: [], submit: [], gpu: [], interval: [] };
  let previous;
  let quads = 0;
  for (let index = 0; index < options.warmup + options.frames; index += 1) {
    const callback = options.mode === 'raf' ? await nextFrame() : undefined;
    const started = performance.now();
    scene.reset({ revision: index + 1, clockMillis: index * 16 });
    crowdScene(scene, options, walkers, textures, index);
    const built = performance.now();
    renderer.submit(scene);
    renderer.render();
    const drawn = performance.now();
    if (options.mode !== 'raf') {
      await renderer.finish();
    }
    const finished = performance.now();
    if (index >= options.warmup) {
      samples.build.push(built - started);
      samples.submit.push(drawn - built);
      if (options.mode === 'raf') {
        // The first callback has no interval before it.
        if (previous !== undefined) {
          samples.interval.push(callback - previous);
        }
      } else {
        samples.gpu.push(finished - drawn);
        samples.frame.push(finished - started);
      }
    }
    previous = callback;
    quads = scene.length;
  }
  await renderer.finish();
  const metrics = renderer.metrics();
  const result = {
    ...options,
    backend: renderer.backend,
    bundle: renderer.bundle,
    adapter: renderer.adapter || (await deviceDescription(renderer.backend)),
    userAgent: navigator.userAgent,
    quads,
    resourceRequests: metrics.resourceRequests,
    samples,
  };
  renderer.destroy();
  return result;
}

window.benchResult = run().catch((error) => ({ error: String(error), stack: error?.stack }));
