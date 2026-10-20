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

const { values } = parseArgs({
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

const BUDGET_60 = 1000 / 60;
const BUDGET_30 = 1000 / 30;
// Callback intervals jitter around the display's refresh interval, so in
// 'raf' mode a frame meets a budget unless it arrives a half interval late,
// that is, unless it missed a refresh.
const RAF_SLACK = 1.5;

function summarise(samples) {
  const sorted = [...samples].sort((left, right) => left - right);
  const at = (percent) =>
    sorted[Math.min(sorted.length, Math.max(1, Math.ceil((percent / 100) * sorted.length))) - 1];
  return { p50: at(50), p95: at(95), p99: at(99), max: sorted[sorted.length - 1] };
}

function within(samples, budget) {
  return (samples.filter((sample) => sample <= budget).length * 100) / samples.length;
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
      within_60fps_percent: within(
        result.samples[key],
        BUDGET_60 * (result.mode === 'raf' ? RAF_SLACK : 1),
      ),
      within_30fps_percent: within(
        result.samples[key],
        BUDGET_30 * (result.mode === 'raf' ? RAF_SLACK : 1),
      ),
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
          ? `frames on time at 60 FPS (interval ≤ 25 ms): ${report.within_60fps_percent.toFixed(1)}%; at 30 FPS (≤ 50 ms): ${report.within_30fps_percent.toFixed(1)}%\n`
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
