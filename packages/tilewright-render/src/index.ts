// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

/**
 * The Tilewright renderer for browsers: the shared Rust renderer compiled to
 * WebAssembly, drawing with WebGPU, or WebGL2 where WebGPU is unavailable.
 * See `docs/WEB.md`.
 */

import { type BindingsModule, type Bundle, loadBindings } from './bindings.js';
import { TilewrightError } from './errors.js';
import {
  MAXIMUM_QUADS,
  QUAD_RECORD_BYTES,
  SceneBuilder,
  type SceneOptions,
  type TextureIdentity,
  checkDigest,
  colorChannels,
} from './scene.js';

export { TilewrightError, type TilewrightErrorCode } from './errors.js';
export {
  MAXIMUM_QUADS,
  QUAD_RECORD_BYTES,
  SceneBuilder,
  SpriteSheet,
  type Color,
  type GlowInput,
  type Layer,
  type Point,
  type QuadInput,
  type Sampling,
  type SceneOptions,
  type SpriteInput,
  type SpriteSheetOptions,
  type TextureIdentity,
  type TextureRect,
} from './scene.js';
export type { Bundle } from './bindings.js';

export type Backend = 'webgpu' | 'webgl2';

export interface RendererOptions {
  /**
   * `auto` (the default) uses WebGPU when the browser offers an adapter and
   * WebGL2 otherwise. An explicit backend never falls back.
   */
  readonly backend?: Backend | 'auto';
  /**
   * The bundle to load: `webgpu` (WebGPU only, about a seventh of the size)
   * or `full` (WebGPU and WebGL2). By default WebGPU loads `webgpu` and
   * WebGL2 loads `full`.
   */
  readonly bundle?: Bundle;
  /** Where to fetch each bundle's `.wasm` file, if not beside its JavaScript. */
  readonly wasm?: Partial<Record<Bundle, string | URL>>;
  /** Drawing-buffer size; the canvas's current size by default. */
  readonly width?: number;
  readonly height?: number;
}

export interface TextureUpload extends Partial<Pick<TextureIdentity, 'revision'>> {
  readonly id: number;
  readonly width: number;
  readonly height: number;
  /** Row-major, unpremultiplied RGBA8 pixels. */
  readonly rgba: Uint8Array | Uint8ClampedArray;
}

/** An uploaded texture. Pass it as a quad's `texture`. */
export interface TextureHandle extends TextureIdentity {
  readonly width: number;
  readonly height: number;
}

/** An image `uploadImage` can read. */
export type ImageSource = CanvasImageSource | ImageData;

/** One frame read back: colour plus the exact picking planes. */
export interface Frame {
  readonly width: number;
  readonly height: number;
  /** Row-major RGBA8. */
  readonly rgba: Uint8Array;
  /** The topmost pickable quad's ID per pixel, 0 for none. */
  readonly semanticIds: Uint32Array;
  /** That quad's depth, −32,768 for none. */
  readonly depth: Int16Array;
  /** That quad's alpha coverage. */
  readonly coverage: Uint8Array;
}

/** Work counts from the most recent frame. */
export interface Metrics {
  readonly framesSubmitted: number;
  readonly resourceRequests: number;
  readonly uploadedBytes: number;
  readonly vertexCount: number;
  readonly vertexAllocationBytes: number;
  readonly skippedFrames: number;
  readonly surfaceReconfigurations: number;
  readonly deviceRecoveries: number;
  /** Bytes of offscreen targets allocated by the frame. */
  readonly targetAllocationBytes: number;
  /** Bytes read back by the frame. */
  readonly readbackBytes: number;
  /** Zero in browsers, where Rust has no clock: time frames with `performance.now()`. */
  readonly frameCpuMicros: number;
}

/** Largest texture side the renderer accepts, in pixels. */
export const MAXIMUM_TEXTURE_SIDE = 8_192;
/**
 * Most bytes one texture may take: its RGBA8 pixels plus a 12-byte header
 * must fit one 16 MiB resource, about 4.2 megapixels.
 */
export const MAXIMUM_TEXTURE_BYTES = 16 * 1024 * 1024;

/** Rejects a drawing-buffer size that is not a positive 32-bit integer. */
function checkTargetSize(width: number, height: number): void {
  for (const [name, side] of [
    ['width', width],
    ['height', height],
  ] as const) {
    if (!Number.isInteger(side) || side < 1 || side > 0xffff_ffff) {
      throw new TilewrightError('invalid-argument', `${name} must be a positive integer`);
    }
  }
}

/** The size an image source draws at, which is not always its `width`. */
function intrinsicSize(image: CanvasImageSource): [number, number] {
  if (typeof HTMLImageElement !== 'undefined' && image instanceof HTMLImageElement) {
    return [image.naturalWidth, image.naturalHeight];
  }
  if (typeof HTMLVideoElement !== 'undefined' && image instanceof HTMLVideoElement) {
    return [image.videoWidth, image.videoHeight];
  }
  if (typeof VideoFrame !== 'undefined' && image instanceof VideoFrame) {
    return [image.displayWidth, image.displayHeight];
  }
  if (typeof SVGImageElement !== 'undefined' && image instanceof SVGImageElement) {
    return [image.width.baseVal.value, image.height.baseVal.value];
  }
  const sized = image as { width: number; height: number };
  return [sized.width, sized.height];
}

/**
 * Rejects a texture size the renderer would refuse, before any pixels are
 * read or copied: `invalid-texture` for a side that is not a positive
 * integer, `limit-exceeded` above `MAXIMUM_TEXTURE_SIDE` or 16 MiB.
 */
function checkTextureSize(width: number, height: number): void {
  for (const [name, side] of [
    ['width', width],
    ['height', height],
  ] as const) {
    if (!Number.isInteger(side) || side < 1) {
      throw new TilewrightError('invalid-texture', `texture ${name} must be a positive integer`);
    }
    if (side > MAXIMUM_TEXTURE_SIDE) {
      throw new TilewrightError(
        'limit-exceeded',
        `texture ${name} ${side} exceeds ${MAXIMUM_TEXTURE_SIDE}`,
      );
    }
  }
  if (width * height * 4 + 12 > MAXIMUM_TEXTURE_BYTES) {
    throw new TilewrightError(
      'limit-exceeded',
      `a ${width} × ${height} texture exceeds ${MAXIMUM_TEXTURE_BYTES} bytes`,
    );
  }
}

const TYPED_ARRAY = Object.getPrototypeOf(Uint8Array.prototype) as object;
const accessor = (name: string): ((view: unknown) => unknown) => {
  const getter = Object.getOwnPropertyDescriptor(TYPED_ARRAY, name)?.get;
  if (getter === undefined) {
    throw new TilewrightError('unsupported', `typed arrays have no ${name}`);
  }
  return (view) => getter.call(view);
};
const bufferOf = accessor('buffer') as (view: unknown) => ArrayBufferLike;
const byteOffsetOf = accessor('byteOffset') as (view: unknown) => number;
const byteLengthOf = accessor('byteLength') as (view: unknown) => number;

/**
 * A plain `Uint8Array` over exactly the bytes of a `Uint8Array` or
 * `Uint8ClampedArray`, read through the built-in accessors: wasm-bindgen
 * sizes its copy from `length`, which a subclass could misreport.
 */
function plainBytes(view: unknown, name: string, code: 'invalid-texture' | 'invalid-scene'): Uint8Array {
  if (!(view instanceof Uint8Array || view instanceof Uint8ClampedArray)) {
    throw new TilewrightError(code, `${name} must be a Uint8Array or Uint8ClampedArray`);
  }
  try {
    return new Uint8Array(bufferOf(view), byteOffsetOf(view), byteLengthOf(view));
  } catch (error) {
    throw new TilewrightError(code, `${name} is not a typed array`, { cause: error });
  }
}

type WebRenderer = Awaited<ReturnType<BindingsModule['WebRenderer']['create']>>;
type WebFrame = ReturnType<WebRenderer['renderReference']>;

async function webgpuAdapterAvailable(): Promise<boolean> {
  const gpu = (globalThis.navigator as { gpu?: { requestAdapter(): Promise<unknown> } } | undefined)
    ?.gpu;
  if (gpu === undefined) {
    return false;
  }
  try {
    return (await gpu.requestAdapter()) != null;
  } catch {
    return false;
  }
}

/**
 * Reads a binding object with `read`, then frees it. If reading throws, a
 * trapped getter leaves the object borrowed, so `free` throws too: the
 * reading error is the one to report.
 */
function readThenFree<T>(object: { free(): void }, read: () => T): T {
  let result: T;
  try {
    result = read();
  } catch (error) {
    try {
      object.free();
    } catch {
      // Ignored: see above.
    }
    throw error;
  }
  object.free();
  return result;
}

function frameFrom(frame: WebFrame): Frame {
  return readThenFree(frame, () => ({
    width: frame.width,
    height: frame.height,
    rgba: frame.rgba,
    semanticIds: frame.semanticIds,
    depth: frame.depth,
    coverage: frame.coverage,
  }));
}

/**
 * Per bundle, set once its WebAssembly module traps. A trap aborts mid-call
 * and leaves the module, which every renderer of that bundle shares, in an
 * undefined state, so those renderers stop with this error: reload the
 * page. The other bundle, a separate module, keeps working.
 */
const traps = new Map<Bundle, TilewrightError>();
/** Per bundle, rejects the waits still pending when its module traps. */
const waiting = new Map<Bundle, Set<(error: TilewrightError) => void>>();

/**
 * Records a trap: from the module's panic hook just before it traps, or
 * from a call that threw `WebAssembly.RuntimeError`.
 */
function reportTrap(bundle: Bundle, detail: string, cause?: unknown): TilewrightError {
  let trap = traps.get(bundle);
  if (trap === undefined) {
    trap = new TilewrightError(
      'internal',
      `the ${bundle} bundle's WebAssembly module trapped; reload the page: ${detail}`,
      { cause },
    );
    traps.set(bundle, trap);
    for (const reject of waiting.get(bundle) ?? []) {
      reject(trap);
    }
    waiting.delete(bundle);
  }
  return trap;
}

/** Converts a binding error, recording a trap. */
function failed(bundle: Bundle, error: unknown): TilewrightError {
  if (error instanceof WebAssembly.RuntimeError) {
    return reportTrap(bundle, error.message, error);
  }
  return traps.get(bundle) ?? TilewrightError.from(error);
}

/**
 * Runs `start`, a binding call, awaits it, then runs `convert` on its
 * result. Every error either throws, synchronously or not, is converted, a
 * `WebAssembly.RuntimeError` recorded as a trap. A trap inside a binding
 * promise never settles it, so a trap in its bundle rejects the wait
 * instead; a result that arrives after a trap is never converted, which
 * would call into the trapped module.
 */
async function untilTrapped<T, R>(
  bundle: Bundle,
  start: () => Promise<T>,
  convert: (result: T) => R,
): Promise<R> {
  const before = traps.get(bundle);
  if (before !== undefined) {
    throw before;
  }
  let stopWaiting = (): void => {};
  const trapped = new Promise<never>((_resolve, reject) => {
    const set = waiting.get(bundle) ?? new Set();
    waiting.set(bundle, set);
    set.add(reject);
    stopWaiting = () => set.delete(reject);
  });
  // Only the race below observes it, and a `start` that throws
  // synchronously never reaches the race.
  trapped.catch(() => {});
  try {
    const result = await Promise.race([start(), trapped]);
    const trap = traps.get(bundle);
    if (trap !== undefined) {
      throw trap;
    }
    return convert(result);
  } catch (error) {
    throw failed(bundle, error);
  } finally {
    stopWaiting();
  }
}

/** A renderer drawing scenes into one canvas. */
export class TilewrightRenderer {
  readonly backend: Backend;
  readonly bundle: Bundle;
  /** The adapter's name as the browser reports it; often empty. */
  readonly adapter: string;
  /**
   * The largest texture side the device accepts, at most
   * `MAXIMUM_TEXTURE_SIDE`; uploads with a larger side fail with
   * `limit-exceeded`.
   */
  readonly maximumTextureSide: number;
  #inner: WebRenderer | undefined;
  #width: number;
  #height: number;

  private constructor(inner: WebRenderer, bundle: Bundle, width: number, height: number) {
    this.#inner = inner;
    this.backend = inner.backend as Backend;
    this.bundle = bundle;
    this.adapter = inner.adapter;
    this.maximumTextureSide = inner.maximumTextureSide;
    this.#width = width;
    this.#height = height;
  }

  /** Creates a renderer on a canvas. */
  static async create(
    canvas: HTMLCanvasElement | OffscreenCanvas,
    options: RendererOptions = {},
  ): Promise<TilewrightRenderer> {
    const requested = options.backend ?? 'auto';
    if (requested !== 'auto' && requested !== 'webgpu' && requested !== 'webgl2') {
      throw new TilewrightError('invalid-argument', `unknown backend ${String(requested)}`);
    }
    const backend: Backend =
      requested === 'auto' ? ((await webgpuAdapterAvailable()) ? 'webgpu' : 'webgl2') : requested;
    const bundle = options.bundle ?? (backend === 'webgpu' ? 'webgpu' : 'full');
    if (bundle !== 'webgpu' && bundle !== 'full') {
      throw new TilewrightError('invalid-argument', `unknown bundle ${String(bundle)}`);
    }
    if (bundle === 'webgpu' && backend === 'webgl2') {
      // With `auto`, the options are valid; the browser lacks WebGPU.
      throw requested === 'auto'
        ? new TilewrightError(
            'backend-unavailable',
            'the browser offers no WebGPU adapter, and the webgpu bundle cannot draw with WebGL2',
          )
        : new TilewrightError('invalid-argument', 'the webgpu bundle cannot draw with WebGL2');
    }
    const width = options.width ?? canvas.width;
    const height = options.height ?? canvas.height;
    checkTargetSize(width, height);
    const trap = traps.get(bundle);
    if (trap !== undefined) {
      throw trap;
    }
    let bindings: BindingsModule;
    try {
      bindings = await loadBindings(bundle, options.wasm?.[bundle]);
      bindings.setTrapHandler((message: string) => {
        reportTrap(bundle, message);
      });
    } catch (error) {
      throw failed(bundle, error);
    }
    return untilTrapped(
      bundle,
      () => bindings.WebRenderer.create(canvas, width, height, backend),
      (inner) => new TilewrightRenderer(inner, bundle, width, height),
    );
  }

  /** The drawing-buffer width in pixels. */
  get width(): number {
    return this.#width;
  }

  /** The drawing-buffer height in pixels. */
  get height(): number {
    return this.#height;
  }

  /** A scene builder sized to this renderer's canvas. */
  scene(options: Omit<SceneOptions, 'width' | 'height'> = {}): SceneBuilder {
    return new SceneBuilder({ ...options, width: this.#width, height: this.#height });
  }

  #renderer(): WebRenderer {
    const trap = traps.get(this.bundle);
    if (trap !== undefined) {
      throw trap;
    }
    if (this.#inner === undefined) {
      throw new TilewrightError('destroyed', 'the renderer was destroyed');
    }
    return this.#inner;
  }

  /**
   * Converts a binding error. After a trap every renderer of the bundle
   * stops: every later call fails with the same `internal` error, and
   * `destroy` only forgets the renderer.
   */
  #failed(error: unknown): TilewrightError {
    return failed(this.bundle, error);
  }

  /** Rejects a texture larger than this device accepts. */
  #checkDeviceTextureSize(width: number, height: number): void {
    if (width > this.maximumTextureSide || height > this.maximumTextureSide) {
      throw new TilewrightError(
        'limit-exceeded',
        `a ${width} × ${height} texture exceeds this device's ${this.maximumTextureSide}-texel side`,
      );
    }
  }

  #call<T>(action: (inner: WebRenderer) => T): T {
    const inner = this.#renderer();
    try {
      return action(inner);
    } catch (error) {
      throw this.#failed(error);
    }
  }

  /**
   * Uploads RGBA8 pixels as texture `id` at `revision` (1 by default). A
   * stored pair never changes: the same pixels are accepted again, and
   * different pixels need a new revision (or `removeTexture` first), else
   * `invalid-texture`. Atlases and sprite sheets are ordinary textures;
   * quads pick their frames.
   */
  uploadTexture(texture: TextureUpload): TextureHandle {
    const revision = texture.revision ?? 1;
    for (const [name, value] of [
      ['id', texture.id],
      ['revision', revision],
    ] as const) {
      if (!Number.isSafeInteger(value) || value < 1) {
        throw new TilewrightError('invalid-texture', `texture ${name} must be a positive safe integer`);
      }
    }
    checkTextureSize(texture.width, texture.height);
    this.#checkDeviceTextureSize(texture.width, texture.height);
    const rgba = plainBytes(texture.rgba, 'rgba', 'invalid-texture');
    if (rgba.byteLength !== texture.width * texture.height * 4) {
      throw new TilewrightError(
        'invalid-texture',
        `rgba must hold ${texture.width * texture.height * 4} bytes`,
      );
    }
    const digest = this.#call((inner) =>
      inner.uploadTexture(texture.id, revision, texture.width, texture.height, rgba),
    );
    // Frozen: builders cache each handle's parsed digest.
    return Object.freeze({
      id: texture.id,
      revision,
      width: texture.width,
      height: texture.height,
      digest,
    });
  }

  /**
   * Uploads an image, such as a decoded sprite sheet. It is drawn into a 2D
   * canvas to read its pixels, so partly transparent texels may lose
   * precision to the canvas's premultiplied storage; upload exact pixels with
   * `uploadTexture`.
   */
  uploadImage(id: number, image: ImageSource, revision = 1): TextureHandle {
    if (typeof ImageData !== 'undefined' && image instanceof ImageData) {
      return this.uploadTexture({
        id,
        revision,
        width: image.width,
        height: image.height,
        rgba: image.data,
      });
    }
    const [width, height] = intrinsicSize(image as CanvasImageSource);
    // Checked before the canvas and its pixels are allocated.
    checkTextureSize(width, height);
    this.#checkDeviceTextureSize(width, height);
    if (typeof OffscreenCanvas === 'undefined') {
      throw new TilewrightError('unsupported', 'reading an image needs OffscreenCanvas');
    }
    let pixels: Uint8ClampedArray;
    try {
      const context = new OffscreenCanvas(width, height).getContext('2d', {
        willReadFrequently: true,
      });
      if (context === null) {
        throw new Error('no 2D canvas context');
      }
      context.drawImage(image as CanvasImageSource, 0, 0, width, height);
      pixels = context.getImageData(0, 0, width, height, { colorSpace: 'srgb' }).data;
    } catch (error) {
      // For example a cross-origin image that taints the canvas.
      throw new TilewrightError('invalid-texture', `the image cannot be read: ${String(error)}`, {
        cause: error,
      });
    }
    return this.uploadTexture({ id, revision, width, height, rgba: pixels });
  }

  /**
   * Forgets an uploaded texture; returns whether it was stored. The whole
   * identity must match, digest included, so a stale handle whose ID and
   * revision now name other pixels removes nothing.
   */
  removeTexture(texture: TextureIdentity): boolean {
    const digest = checkDigest(texture);
    return this.#call((inner) => inner.removeTexture(texture.id, texture.revision, digest));
  }

  /** Validates a scene and makes it the one `render` and `readFrame` draw. */
  submit(scene: SceneBuilder): void {
    const options = scene.options;
    // Checked again here: the bindings coerce numbers silently, and the
    // records must stay within the scene limit before they are copied.
    checkTargetSize(options.width, options.height);
    for (const [name, value, minimum] of [
      ['revision', options.revision, 1],
      ['clockMillis', options.clockMillis, 0],
      ['scaleMilli', options.scaleMilli, 1],
    ] as const) {
      if (!Number.isSafeInteger(value) || value < minimum) {
        throw new TilewrightError('invalid-argument', `${name} must be a safe integer`);
      }
    }
    if (options.scaleMilli > 0xffff_ffff) {
      throw new TilewrightError('invalid-argument', 'scaleMilli must fit in 32 bits');
    }
    const records = plainBytes(scene.records(), 'records', 'invalid-scene');
    if (records.byteLength % QUAD_RECORD_BYTES !== 0) {
      throw new TilewrightError('invalid-scene', 'records must be whole packed quad records');
    }
    if (records.byteLength > MAXIMUM_QUADS * QUAD_RECORD_BYTES) {
      throw new TilewrightError('limit-exceeded', `a scene holds at most ${MAXIMUM_QUADS} quads`);
    }
    // Read as exactly four numbers: `options` may come from a subclass.
    const clear = Float32Array.from(colorChannels(options.clearColor, 'clearColor'));
    this.#call((inner) =>
      inner.submit(
        options.revision,
        options.clockMillis,
        options.width,
        options.height,
        options.scaleMilli,
        clear,
        records,
      ),
    );
  }

  /**
   * Draws the submitted scene into the canvas. Returns false when the
   * browser skipped the frame (`surface-unavailable`); throws otherwise.
   */
  render(): boolean {
    try {
      this.#call((inner) => inner.render());
      return true;
    } catch (error) {
      if (error instanceof TilewrightError && error.code === 'surface-unavailable') {
        return false;
      }
      throw error;
    }
  }

  /** Resizes the drawing buffer; submit a scene of the new size next. */
  resize(width: number, height: number): void {
    checkTargetSize(width, height);
    this.#call((inner) => inner.resize(width, height));
    this.#width = width;
    this.#height = height;
  }

  /** Renders the submitted scene offscreen and reads every plane back. */
  async readFrame(): Promise<Frame> {
    return untilTrapped(
      this.bundle,
      () => this.#renderer().readFrame(),
      (frame) => frameFrom(frame as WebFrame),
    );
  }

  /**
   * Resolves once the GPU has finished every frame submitted so far,
   * presented or read back: await it after `render` to measure a frame's
   * GPU time.
   */
  async finish(): Promise<void> {
    await untilTrapped(this.bundle, () => this.#renderer().finish(), () => undefined);
  }

  /**
   * Renders the submitted scene with the CPU reference renderer: the frame
   * every backend matches within one 8-bit step, with exact picking planes.
   */
  renderReference(): Frame {
    return this.#call((inner) => frameFrom(inner.renderReference()));
  }

  /** Work counts from the most recent frame. */
  metrics(): Metrics {
    return this.#call((inner) => {
      const metrics = inner.metrics();
      return readThenFree(metrics, () => ({
        framesSubmitted: metrics.framesSubmitted,
        resourceRequests: metrics.resourceRequests,
        uploadedBytes: metrics.uploadedBytes,
        vertexCount: metrics.vertexCount,
        vertexAllocationBytes: metrics.vertexAllocationBytes,
        skippedFrames: metrics.skippedFrames,
        surfaceReconfigurations: metrics.surfaceReconfigurations,
        deviceRecoveries: metrics.deviceRecoveries,
        targetAllocationBytes: metrics.targetAllocationBytes,
        readbackBytes: metrics.readbackBytes,
        frameCpuMicros: metrics.frameCpuMicros,
      }));
    });
  }

  /** Releases the GPU device and every texture. The renderer is unusable afterwards. */
  destroy(): void {
    // A readback still in flight stops waiting, rejects with `destroyed`
    // and releases the device; otherwise the device is released now.
    const inner = this.#inner;
    this.#inner = undefined;
    if (inner === undefined || traps.has(this.bundle)) {
      // A trapped call keeps the Rust object borrowed: freeing it would throw.
      return;
    }
    try {
      inner.destroy();
      inner.free();
    } catch (error) {
      // Never thrown: only a trap, which `#failed` records, or the object it
      // left borrowed, which nothing can free, end up here.
      this.#failed(error);
    }
  }
}
