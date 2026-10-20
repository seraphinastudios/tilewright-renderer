// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

/**
 * Loads one of the two WebAssembly bundles: `webgpu`, WebGPU only and small,
 * or `full`, WebGPU and WebGL2. Each loads at most once per page.
 */

import type * as Bindings from '../wasm/full/tilewright_render_web.js';

export type BindingsModule = typeof Bindings;
export type Bundle = 'webgpu' | 'full';

const loaded = new Map<Bundle, Promise<BindingsModule>>();

/**
 * Loads and initialises a bundle. `wasm` overrides where its `.wasm` file
 * is fetched from; by default it sits beside the bundle's JavaScript.
 */
export function loadBindings(bundle: Bundle, wasm?: string | URL): Promise<BindingsModule> {
  let promise = loaded.get(bundle);
  if (promise === undefined) {
    // Both bundles expose the same API; only their internal exports differ.
    const module: Promise<BindingsModule> =
      bundle === 'webgpu'
        ? (import('../wasm/webgpu/tilewright_render_web.js') as Promise<unknown> as Promise<BindingsModule>)
        : import('../wasm/full/tilewright_render_web.js');
    promise = module.then(async (bindings) => {
      await bindings.default(wasm === undefined ? undefined : { module_or_path: wasm });
      return bindings;
    });
    // A failed load may be retried, for example once the network returns.
    promise.catch(() => loaded.delete(bundle));
    loaded.set(bundle, promise);
  }
  return promise;
}
