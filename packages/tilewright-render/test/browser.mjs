// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// Launches headless Chromium for the browser tests. TILEWRIGHT_CHROMIUM
// names an executable; otherwise Playwright's own browsers are used when
// PLAYWRIGHT_BROWSERS_PATH is set, and by default the installed Google
// Chrome. Nothing is downloaded.

import { chromium } from 'playwright-core';

/**
 * Flags that run WebGPU and WebGL2 on SwiftShader, Chromium's CPU
 * implementation, whether or not the machine has a GPU, so every machine
 * renders the same way as CI. SwiftShader must also back Vulkan and ANGLE:
 * with any other compositor, presenting to a WebGPU canvas destroys the
 * device in headless mode.
 */
export const SOFTWARE_FLAGS = [
  '--enable-unsafe-webgpu',
  '--enable-features=Vulkan',
  '--use-vulkan=swiftshader',
  '--use-angle=swiftshader',
  '--use-webgpu-adapter=swiftshader',
];

/**
 * Flags that let headless Chrome draw WebGL2 on the machine's GPU, for the
 * frame-time harness. Its WebGPU stays on SwiftShader; docs/FRAME-TIME.md
 * describes a hardware WebGPU setup.
 */
export const GPU_FLAGS = ['--enable-unsafe-webgpu', '--ignore-gpu-blocklist', '--enable-gpu'];

export function launch(flags = SOFTWARE_FLAGS, options = {}) {
  const settings = { ...options, args: flags };
  if (process.env.TILEWRIGHT_CHROMIUM) {
    return chromium.launch({ ...settings, executablePath: process.env.TILEWRIGHT_CHROMIUM });
  }
  if (process.env.PLAYWRIGHT_BROWSERS_PATH) {
    return chromium.launch(settings);
  }
  return chromium.launch({ ...settings, channel: 'chrome' });
}
