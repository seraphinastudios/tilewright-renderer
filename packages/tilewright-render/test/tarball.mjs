// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// Checks a packed release tarball as a consumer gets it: installs it into an
// empty project with npm, resolves the package by name through its
// `exports`, and draws one sprite from the installed files in headless
// Chromium with the `full` bundle on WebGL2 and the `webgpu` bundle on
// WebGPU, against the CPU reference. tools/package-release.sh runs it on the
// tarball it packs, before SHA256SUMS is written.
//
//   node test/tarball.mjs PATH.tgz
//
// Run it from an installed package directory (`npm ci`), which provides
// playwright-core. TILEWRIGHT_REQUIRE_WEBGPU=0 skips, rather than fails,
// WebGPU where the browser offers no adapter, as in the browser tests.

import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { copyFile, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { launch } from './browser.mjs';
import { serve } from './server.mjs';

const NAME = '@seraphinastudios/tilewright-render';
const PAGE = fileURLToPath(new URL('consumer/', import.meta.url));

const tarball = process.argv[2];
if (!tarball || process.argv.length !== 3) {
  console.error('usage: node test/tarball.mjs PATH.tgz');
  process.exit(2);
}

const project = await mkdtemp(join(tmpdir(), 'tilewright-render-consumer-'));
let browser;
let site;
try {
  await writeFile(
    join(project, 'package.json'),
    `${JSON.stringify({ name: 'tarball-consumer', private: true, type: 'module' })}\n`,
  );
  // The tarball has no dependencies, so nothing is fetched; no install
  // script runs.
  execFileSync(
    'npm',
    ['install', '--ignore-scripts', '--no-audit', '--no-fund', '--offline', '--no-package-lock', resolve(tarball)],
    { cwd: project, stdio: ['ignore', 'inherit', 'inherit'] },
  );

  // Resolve the package as Node and bundlers do, through `exports`.
  const entry = execFileSync(
    process.execPath,
    ['--input-type=module', '--eval', `console.log(import.meta.resolve(${JSON.stringify(NAME)}))`],
    { cwd: project, encoding: 'utf8' },
  ).trim();
  const entryPath = fileURLToPath(entry);
  assert.ok(entryPath.startsWith(join(project, 'node_modules') + sep), `${NAME} resolved outside the project: ${entry}`);
  const types = JSON.parse(await readFile(join(project, 'node_modules', NAME, 'package.json'), 'utf8')).types;
  await readFile(join(project, 'node_modules', NAME, types));

  const importMap = JSON.stringify({ imports: { [NAME]: `./${relative(project, entryPath).split(sep).join('/')}` } });
  const html = (await readFile(join(PAGE, 'index.html'), 'utf8')).replace('IMPORT_MAP', importMap);
  await writeFile(join(project, 'index.html'), html);
  await copyFile(join(PAGE, 'consumer.js'), join(project, 'consumer.js'));

  site = await serve(project);
  browser = await launch();
  for (const backend of ['webgl2', 'webgpu']) {
    const page = await browser.newPage();
    const messages = [];
    page.on('console', (message) => messages.push(`${message.type()}: ${message.text()}`));
    page.on('pageerror', (error) => messages.push(`pageerror: ${error.message}`));
    let result;
    try {
      await page.goto(`${site.base}/index.html?backend=${backend}`);
      result = await page.evaluate(() => window.consumerResult);
    } finally {
      await page.close();
    }
    if (result?.unavailable && backend === 'webgpu' && process.env.TILEWRIGHT_REQUIRE_WEBGPU === '0') {
      console.log(`tarball: skipped webgpu, no adapter: ${result.error}`);
      continue;
    }
    const detail = `${JSON.stringify(result)}\n${messages.join('\n')}`;
    assert.ok(result && result.error === undefined, `${backend}: ${detail}`);
    assert.equal(result.backend, backend, detail);
    assert.equal(result.bundle, backend === 'webgpu' ? 'webgpu' : 'full', detail);
    assert.equal(result.covered, 64, `${backend}: the sprite covers ${result.covered} pixels, not 64`);
    assert.ok(result.semanticEqual, `${backend}: the semantic plane differs from the reference`);
    assert.ok(result.rgbaDifference <= 1, `${backend}: RGBA differs from the reference by ${result.rgbaDifference}`);
    console.log(`tarball: ${backend} drew with the ${result.bundle} bundle from the installed package`);
  }
} finally {
  await browser?.close();
  site?.server.close();
  await rm(project, { recursive: true, force: true });
}
