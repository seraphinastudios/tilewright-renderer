#!/usr/bin/env node
// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// Frame-time harness for browsers: renders the crowd scene in headless
// Chromium and reports frame-time percentiles against the 60 and 30 FPS
// budgets. See docs/FRAME-TIME.md.
//
//   npm run bench -- --backend webgpu --characters 1000 --frames 300
//
// Options: --backend webgpu|webgl2|all (all), --width (1920), --height
// (1080), --characters (1000), --sheets atlas|layers (atlas), --frames (300),
// --warmup (30), --mode serial|raf (serial), --gpu (use the GPU for WebGL2
// rather than SwiftShader), --headed, --chrome-flag FLAG (repeatable) and
// --json.

import { parseArgs } from 'node:util';
import { GPU_FLAGS, SOFTWARE_FLAGS, launch } from '../test/browser.mjs';
import { serve } from '../test/server.mjs';

const USAGE = `usage: npm run bench -- [--backend webgpu|webgl2|all] [--width N] [--height N]
  [--characters N] [--sheets atlas|layers] [--frames N] [--warmup N] [--mode serial|raf]
  [--gpu] [--headed] [--chrome-flag FLAG]... [--json]`;

/** Reports a bad option with the usage and exits non-zero, as the native harness does. */
function refuse(message) {
  console.error(`${message}\n${USAGE}`);
  process.exit(2);
}

let parsed;
try {
  parsed = parseArgs({
    options: {
    backend: { type: 'string', default: 'all' },
    width: { type: 'string', default: '1920' },
    height: { type: 'string', default: '1080' },
    characters: { type: 'string', default: '1000' },
    sheets: { type: 'string', default: 'atlas' },
    frames: { type: 'string', default: '300' },
    warmup: { type: 'string', default: '30' },
    mode: { type: 'string', default: 'serial' },
    gpu: { type: 'boolean', default: false },
    headed: { type: 'boolean', default: false },
    'chrome-flag': { type: 'string', multiple: true, default: [] },
    json: { type: 'boolean', default: false },
    },
  });
} catch (error) {
  refuse(error.message);
}
const { values } = parsed;
for (const [name, allowed] of [
  ['backend', ['webgpu', 'webgl2', 'all']],
  ['sheets', ['atlas', 'layers']],
  ['mode', ['serial', 'raf']],
]) {
  if (!allowed.includes(values[name])) {
    refuse(`--${name} must be one of ${allowed.join(', ')}`);
  }
}
for (const [name, minimum] of [
  ['width', 1],
  ['height', 1],
  ['characters', 0],
  ['frames', 1],
  // An interval needs a callback before it.
  ['warmup', values.mode === 'raf' ? 1 : 0],
]) {
  const text = values[name];
  if (!/^[0-9]+$/.test(text) || Number(text) < minimum || Number(text) > 0xffff_ffff) {
    refuse(`--${name} must be an integer from ${minimum} to ${0xffff_ffff}`);
  }
}

const REFRESH = 1000 / 60;
/**
 * Whether a sample meets a budget of `refreshes` 60 Hz refreshes. A serial
 * frame time must fit the budget. A 'raf' interval jitters around whole
 * refreshes, so it is on time unless it arrives at least half a refresh
 * late, that is, unless it missed a refresh: under 1.5 refreshes (25 ms) at
 * 60 FPS and under 2.5 (41.7 ms) at 30 FPS. Counting in refreshes keeps a
 * coarsened 50.0 ms interval, three refreshes, from passing at 30 FPS.
 */
function onTime(mode, refreshes) {
  return mode === 'raf'
    ? (sample) => sample < (refreshes + 0.5) * REFRESH
    : (sample) => sample <= refreshes * REFRESH;
}

function summarise(samples) {
  if (samples.length === 0) {
    return { p50: NaN, p95: NaN, p99: NaN, max: NaN };
  }
  const sorted = [...samples].sort((left, right) => left - right);
  const at = (percent) =>
    sorted[Math.min(sorted.length, Math.max(1, Math.ceil((percent / 100) * sorted.length))) - 1];
  return { p50: at(50), p95: at(95), p99: at(99), max: sorted[sorted.length - 1] };
}

function within(samples, test) {
  return samples.length === 0 ? NaN : (samples.filter(test).length * 100) / samples.length;
}

/** The middle sample, or NaN for none. */
function median(samples) {
  return summarise(samples).p50;
}

const backends = values.backend === 'all' ? ['webgpu', 'webgl2'] : [values.backend];
const site = await serve();
const flags = [...(values.gpu ? GPU_FLAGS : SOFTWARE_FLAGS), ...values['chrome-flag']];
const browser = await launch(flags, { headless: !values.headed });
const reports = [];
let failed = false;
try {
  for (const backend of backends) {
    const page = await browser.newPage();
    const query = new URLSearchParams({
      backend,
      width: values.width,
      height: values.height,
      characters: values.characters,
      sheets: values.sheets,
      frames: values.frames,
      warmup: values.warmup,
      mode: values.mode,
    });
    await page.goto(`${site.base}/bench/frame-time.html?${query}`);
    // A hung page reports an error instead of stalling the harness.
    const result = await Promise.race([
      page.evaluate(() => window.benchResult),
      new Promise((resolve) =>
        setTimeout(() => resolve({ error: 'timed out after 10 minutes' }), 600_000).unref(),
      ),
    ]);
    await page.close();
    if (result.error) {
      console.error(`${backend}: ${result.error}`);
      failed = true;
      continue;
    }
    const key = result.mode === 'raf' ? 'interval' : 'frame';
    if (result.samples[key].length === 0) {
      console.error(`${backend}: no ${key} samples were recorded`);
      failed = true;
      continue;
    }
    // The raf thresholds count 60 Hz refreshes, the headless default.
    const warmupInterval = median(result.samples.warmupInterval ?? []);
    if (result.mode === 'raf' && !(warmupInterval >= 15 && warmupInterval <= 18.5)) {
      console.error(
        `warning: the median warm-up interval was ${warmupInterval.toFixed(1)} ms, not about 16.7 ms; the on-time percentages assume a 60 Hz requestAnimationFrame clock`,
      );
    }
    const cpu = result.samples.build.map((build, index) => build + result.samples.submit[index]);
    const report = {
      backend: result.backend,
      bundle: result.bundle,
      adapter: result.adapter,
      userAgent: result.userAgent,
      width: result.width,
      height: result.height,
      characters: result.characters,
      sheets: result.sheets,
      quads: result.quads,
      mode: result.mode,
      frames: result.samples[key].length,
      [`${key}_ms`]: summarise(result.samples[key]),
      cpu_ms: summarise(cpu),
      build_ms: summarise(result.samples.build),
      submit_ms: summarise(result.samples.submit),
      ...(result.mode === 'raf' ? {} : { gpu_wait_ms: summarise(result.samples.gpu) }),
      within_60fps_percent: within(result.samples[key], onTime(result.mode, 1)),
      within_30fps_percent: within(result.samples[key], onTime(result.mode, 2)),
    };
    reports.push(report);
    if (!values.json) {
      console.log(
        `${report.backend} (${report.bundle} bundle, adapter ${JSON.stringify(report.adapter)}), ${report.userAgent}`,
      );
      console.log(
        `scene: ${report.width}×${report.height}, ${report.characters} characters × 3 layers, ${report.sheets} sheets, ${report.quads} quads; ${report.frames} frames, ${report.mode} mode`,
      );
      console.log('                     p50      p95      p99      max   (ms)');
      const rows = [
        [key === 'frame' ? 'frame (CPU + GPU)' : 'frame interval', report[`${key}_ms`]],
        ['CPU', report.cpu_ms],
        ['  scene build', report.build_ms],
        ['  submit and draw', report.submit_ms],
      ];
      if (report.gpu_wait_ms) {
        rows.push(['GPU wait', report.gpu_wait_ms]);
      }
      for (const [name, stats] of rows) {
        console.log(
          `${name.padEnd(18)} ${[stats.p50, stats.p95, stats.p99, stats.max].map((value) => value.toFixed(2).padStart(8)).join(' ')}`,
        );
      }
      console.log(
        report.mode === 'raf'
          ? `frames on time at 60 FPS (interval < 25 ms): ${report.within_60fps_percent.toFixed(1)}%; at 30 FPS (< 41.7 ms): ${report.within_30fps_percent.toFixed(1)}%\n`
          : `frames within 16.7 ms (60 FPS): ${report.within_60fps_percent.toFixed(1)}%; within 33.3 ms (30 FPS): ${report.within_30fps_percent.toFixed(1)}%\n`,
      );
    }
  }
} finally {
  await browser.close();
  site.server.close();
}
if (values.json) {
  console.log(JSON.stringify(reports));
}
process.exitCode = failed ? 1 : 0;
