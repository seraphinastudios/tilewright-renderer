// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

/**
 * The Tilewright renderer for browsers: the shared Rust renderer compiled to
 * WebAssembly, drawing with WebGPU, or WebGL2 where WebGPU is unavailable.
 * See `docs/WEB.md`.
 */

import { type BindingsModule, type Bundle, loadBindings } from './bindings.js';
import { TilewrightError } from './errors.js';
import { SceneBuilder, type SceneOptions, type TextureIdentity } from './scene.js';

export { TilewrightError, type TilewrightErrorCode } from './errors.js';
export {
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
   * The bundle to load: `webgpu` (WebGPU only, about an eighth of the size)
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
  /** SHA-256 of the texture's envelope, in hexadecimal. */
  readonly digest: string;
}

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

function frameFrom(frame: WebFrame): Frame {
  try {
    return {
      width: frame.width,
      height: frame.height,
      rgba: frame.rgba,
      semanticIds: frame.semanticIds,
      depth: frame.depth,
      coverage: frame.coverage,
    };
  } finally {
    frame.free();
  }
}

/** A renderer drawing scenes into one canvas. */
export class TilewrightRenderer {
  readonly backend: Backend;
  readonly bundle: Bundle;
  /** The adapter's name as the browser reports it; often empty. */
  readonly adapter: string;
  #inner: WebRenderer | undefined;
  #width: number;
  #height: number;

  private constructor(inner: WebRenderer, bundle: Bundle, width: number, height: number) {
    this.#inner = inner;
    this.backend = inner.backend as Backend;
    this.bundle = bundle;
    this.adapter = inner.adapter;
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
      throw new TilewrightError('invalid-argument', 'the webgpu bundle cannot draw with WebGL2');
    }
    const width = options.width ?? canvas.width;
    const height = options.height ?? canvas.height;
    try {
      const bindings = await loadBindings(bundle, options.wasm?.[bundle]);
      const inner = await bindings.WebRenderer.create(canvas, width, height, backend);
      return new TilewrightRenderer(inner, bundle, width, height);
    } catch (error) {
      throw TilewrightError.from(error);
    }
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
    if (this.#inner === undefined) {
      throw new TilewrightError('invalid-argument', 'the renderer was destroyed');
    }
    return this.#inner;
  }

  #call<T>(action: (inner: WebRenderer) => T): T {
    const inner = this.#renderer();
    try {
      return action(inner);
    } catch (error) {
      throw TilewrightError.from(error);
    }
  }

  /**
   * Uploads RGBA8 pixels as texture `id` at `revision` (1 by default),
   * replacing any earlier upload of that pair. Atlases and sprite sheets are
   * ordinary textures; quads pick their frames.
   */
  uploadTexture(texture: TextureUpload): TextureHandle {
    const revision = texture.revision ?? 1;
    const rgba =
      texture.rgba instanceof Uint8Array
        ? texture.rgba
        : new Uint8Array(texture.rgba.buffer, texture.rgba.byteOffset, texture.rgba.byteLength);
    const digest = this.#call((inner) =>
      inner.uploadTexture(texture.id, revision, texture.width, texture.height, rgba),
    );
    return { id: texture.id, revision, width: texture.width, height: texture.height, digest };
  }

  /**
   * Uploads an image, such as a decoded sprite sheet. It is drawn into a 2D
   * canvas to read its pixels, so partly transparent texels may lose
   * precision to the canvas's premultiplied storage; upload exact pixels with
   * `uploadTexture`.
   */
  uploadImage(id: number, image: CanvasImageSource & { width: number; height: number }, revision = 1): TextureHandle {
    const width = image.width;
    const height = image.height;
    const canvas = new OffscreenCanvas(width, height);
    const context = canvas.getContext('2d', { willReadFrequently: true });
    if (context === null) {
      throw new TilewrightError('unsupported', 'no 2D canvas context to read the image');
    }
    context.drawImage(image, 0, 0);
    const pixels = context.getImageData(0, 0, width, height, { colorSpace: 'srgb' }).data;
    return this.uploadTexture({ id, revision, width, height, rgba: pixels });
  }

  /** Forgets an uploaded texture; returns whether it was stored. */
  removeTexture(texture: TextureIdentity): boolean {
    return this.#call((inner) => inner.removeTexture(texture.id, texture.revision));
  }

  /** Validates a scene and makes it the one `render` and `readFrame` draw. */
  submit(scene: SceneBuilder): void {
    const options = scene.options;
    const clear = new Float32Array(options.clearColor);
    this.#call((inner) =>
      inner.submit(
        options.revision,
        options.clockMillis,
        options.width,
        options.height,
        options.scaleMilli,
        clear,
        scene.records(),
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
    this.#call((inner) => inner.resize(width, height));
    this.#width = width;
    this.#height = height;
  }

  /** Renders the submitted scene offscreen and reads every plane back. */
  async readFrame(): Promise<Frame> {
    const inner = this.#renderer();
    try {
      return frameFrom((await inner.readFrame()) as WebFrame);
    } catch (error) {
      throw TilewrightError.from(error);
    }
  }

  /**
   * Renders the submitted scene with the CPU reference renderer: the frame
   * every backend matches within one 8-bit step, with exact picking planes.
   */
  renderReference(): Frame {
    return frameFrom(this.#call((inner) => inner.renderReference()));
  }

  /** Work counts from the most recent frame. */
  metrics(): Metrics {
    const metrics = this.#call((inner) => inner.metrics());
    try {
      return {
        framesSubmitted: metrics.framesSubmitted,
        resourceRequests: metrics.resourceRequests,
        uploadedBytes: metrics.uploadedBytes,
        vertexCount: metrics.vertexCount,
        vertexAllocationBytes: metrics.vertexAllocationBytes,
        skippedFrames: metrics.skippedFrames,
        surfaceReconfigurations: metrics.surfaceReconfigurations,
      };
    } finally {
      metrics.free();
    }
  }

  /** Releases the GPU device and every texture. The renderer is unusable afterwards. */
  destroy(): void {
    this.#inner?.free();
    this.#inner = undefined;
  }
}
