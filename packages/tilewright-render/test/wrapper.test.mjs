// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

// The wrapper's error conversion, against a fake binding: every failure of a
// binding call or of converting its result reaches callers as a typed
// TilewrightError, and a WebAssembly trap stops the bundle's renderers.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { TilewrightError, TilewrightRenderer } from '../dist/index.js';

/** A binding object with the members the wrapper reads, overridable. */
function fakeBinding(overrides = {}) {
  return {
    backend: 'webgl2',
    adapter: 'fake',
    maximumTextureSide: 8_192,
    render() {},
    destroy() {},
    free() {},
    ...overrides,
  };
}

// The constructor is private to TypeScript only; tests build renderers
// around fake bindings directly.
const renderer = (binding, bundle) => new TilewrightRenderer(binding, bundle, 8, 8);

test('a frame that cannot be converted rejects with a typed error', async () => {
  const frame = {
    width: 8,
    height: 8,
    get rgba() {
      throw new RangeError('Array buffer allocation failed');
    },
    free() {},
  };
  const subject = renderer(fakeBinding({ readFrame: async () => frame }), 'webgpu');
  await assert.rejects(subject.readFrame(), (error) => {
    assert.ok(error instanceof TilewrightError);
    assert.equal(error.code, 'internal');
    return true;
  });
  // Not a trap: the renderer keeps working.
  assert.doesNotThrow(() => subject.render());
});

test('a synchronous trap in a binding call stops every renderer of its bundle', async () => {
  const trapping = renderer(
    fakeBinding({
      readFrame() {
        throw new WebAssembly.RuntimeError('unreachable');
      },
    }),
    'full',
  );
  const sibling = renderer(fakeBinding(), 'full');
  await assert.rejects(trapping.readFrame(), (error) => {
    assert.ok(error instanceof TilewrightError);
    assert.equal(error.code, 'internal');
    assert.match(error.message, /full bundle's WebAssembly module trapped/);
    return true;
  });
  assert.throws(() => sibling.render(), (error) => error instanceof TilewrightError && error.code === 'internal');
  assert.doesNotThrow(() => trapping.destroy());
});

test('a trap in a frame getter stops the bundle, though free then throws', async () => {
  const frame = {
    width: 8,
    height: 8,
    get rgba() {
      throw new WebAssembly.RuntimeError('unreachable');
    },
    free() {
      throw new Error('attempted to take ownership of Rust value while it was borrowed');
    },
  };
  const subject = renderer(fakeBinding({ readFrame: async () => frame }), 'webgpu');
  const sibling = renderer(fakeBinding(), 'webgpu');
  await assert.rejects(subject.readFrame(), /webgpu bundle's WebAssembly module trapped/);
  assert.throws(() => sibling.render(), (error) => error instanceof TilewrightError && error.code === 'internal');
});

test('a frame that arrives after a trap is never converted', async () => {
  // Bundles are only keys here: a fresh one, untouched by the tests above.
  const bundle = 'late';
  let resolveLate;
  const touched = [];
  const late = new Proxy(
    {},
    {
      get(_target, key) {
        touched.push(String(key));
        return key === 'free' ? () => {} : undefined;
      },
    },
  );
  const waiting = renderer(
    fakeBinding({
      readFrame: () =>
        new Promise((resolve) => {
          resolveLate = resolve;
        }),
    }),
    bundle,
  );
  const trapping = renderer(
    fakeBinding({
      render() {
        throw new WebAssembly.RuntimeError('unreachable');
      },
    }),
    bundle,
  );
  const pending = waiting.readFrame();
  assert.throws(() => trapping.render(), (error) => error instanceof TilewrightError && error.code === 'internal');
  await assert.rejects(pending, /late bundle's WebAssembly module trapped/);
  resolveLate(late);
  await new Promise((resolve) => setTimeout(resolve, 0));
  // Resolving a promise asks for `then`; nothing else may be read.
  assert.deepEqual(touched.filter((key) => key !== 'then'), []);
});

test('bytes reach the binding as plain views of their real length, and handles are frozen', () => {
  const received = {};
  const subject = renderer(
    fakeBinding({
      uploadTexture(_id, _revision, _width, _height, rgba) {
        received.rgba = rgba;
        return '0f'.repeat(32);
      },
      submit(_revision, _clock, _width, _height, _scale, _clear, records) {
        received.records = records;
      },
    }),
    'bytes',
  );
  // wasm-bindgen sizes its copy from `length`, which a subclass can forge.
  class Forged extends Uint8Array {
    get length() {
      return 1_000_000_000;
    }
    get byteLength() {
      return 1_000_000_000;
    }
  }
  const handle = subject.uploadTexture({ id: 1, width: 1, height: 1, rgba: new Forged(4) });
  assert.equal(Object.getPrototypeOf(received.rgba), Uint8Array.prototype);
  assert.equal(received.rgba.length, 4);
  assert.ok(Object.isFrozen(handle));
  const scene = subject.scene().sprite({ id: 1, texture: handle, x: 0, y: 0, width: 8, height: 8 });
  const records = scene.records();
  scene.records = () => new Forged(records.buffer, 0, records.byteLength);
  subject.submit(scene);
  assert.equal(Object.getPrototypeOf(received.records), Uint8Array.prototype);
  assert.equal(received.records.length, records.byteLength);
});
