// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// Launches headless Chromium for the browser tests. Playwright's own
// browsers are used when PLAYWRIGHT_BROWSERS_PATH is set; otherwise
// TILEWRIGHT_CHROMIUM names an executable, and by default the installed
// Google Chrome is used. Nothing is downloaded.

import { chromium } from 'playwright-core';

/**
 * Flags that expose WebGPU in headless Chromium, where SwiftShader serves
 * WebGPU and WebGL2 whether or not a GPU is present. Without GPU
 * compositing (`--enable-gpu`), presenting to a WebGPU canvas destroys the
 * device in headless mode.
 */
export const CHROMIUM_FLAGS = ['--enable-unsafe-webgpu', '--ignore-gpu-blocklist', '--enable-gpu'];

export function launch(extraFlags = []) {
  const args = [...CHROMIUM_FLAGS, ...extraFlags];
  if (process.env.TILEWRIGHT_CHROMIUM) {
    return chromium.launch({ executablePath: process.env.TILEWRIGHT_CHROMIUM, args });
  }
  if (process.env.PLAYWRIGHT_BROWSERS_PATH) {
    return chromium.launch({ args });
  }
  return chromium.launch({ channel: 'chrome', args });
}
