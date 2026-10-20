// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

/**
 * Scene building: textured quads packed into the fixed-size little-endian
 * records the WebAssembly renderer decodes (`docs/WEB.md`). A builder keeps
 * its buffer between frames, so rebuilding a scene of thousands of quads
 * every frame allocates nothing once the buffer has grown.
 */

import { TilewrightError } from './errors.js';

/** Texels of one frame of an atlas or sprite sheet. */
export interface TextureRect {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

/**
 * The full identity of an uploaded texture: its ID, revision and the
 * SHA-256 `uploadTexture` returned. Scenes carry all three, so a stale
 * identity is refused rather than drawing other pixels.
 */
export interface TextureIdentity {
  readonly id: number;
  readonly revision: number;
  /** 64 hexadecimal digits. */
  readonly digest: string;
}

export type Sampling = 'nearest' | 'linear';
export type Layer = 'world' | 'effect' | 'interface';
export type Color = readonly [number, number, number, number];
export type Point = readonly [number, number];

/** A coloured halo around the texture's silhouette. */
export interface GlowInput {
  /** Halo colour; alpha is its strength. */
  readonly color: Color;
  /** Radius in frame-relative texture coordinates, per axis, 0–1. */
  readonly radius: Point;
}

/** A textured quadrilateral. */
export interface QuadInput {
  /** Picking identity, 1 to 2³² − 1. Several quads of one object share it. */
  readonly id: number;
  /** Distinguishes quads sharing an identity, 0–65,535. */
  readonly part?: number;
  readonly texture: TextureIdentity;
  /** The frame sampled; the whole texture when absent. */
  readonly frame?: TextureRect;
  /** Corners in pixels: top-left, top-right, bottom-right, bottom-left. */
  readonly positions: readonly [Point, Point, Point, Point];
  /** Frame-relative texture coordinates per corner; the whole frame by default. */
  readonly uvs?: readonly [Point, Point, Point, Point];
  /** −32,768 to 32,767. */
  readonly depth?: number;
  /** Painter order within a depth, 0 to 2³² − 1. */
  readonly order?: number;
  /** Multiplied with the texture; alpha scales opacity. White by default. */
  readonly color?: Color;
  /** 0 keeps colour, 1 is fully grey. */
  readonly grayscale?: number;
  readonly glow?: GlowInput;
  /** Whether the quad is reported by picking; true unless it glows. */
  readonly pickable?: boolean;
  readonly sampling?: Sampling;
  readonly layer?: Layer;
}

/** An axis-aligned quad with a positive width and height. */
export interface SpriteInput extends Omit<QuadInput, 'positions' | 'uvs' | 'glow'> {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

export interface SceneOptions {
  /** Viewport size in pixels; it must match the renderer's canvas. */
  readonly width: number;
  readonly height: number;
  readonly clearColor?: Color;
  /** Logical scale in thousandths, 1–8,000. */
  readonly scaleMilli?: number;
  /** Positive scene revision. */
  readonly revision?: number;
  /** Clock for timed effects, in milliseconds. */
  readonly clockMillis?: number;
}

/** Bytes per packed quad record: 39 words, then the texture's digest. */
export const QUAD_RECORD_BYTES = 188;
const DIGEST_OFFSET = 156;
/** Most quads one scene may hold: the renderer's scene limit. */
export const MAXIMUM_QUADS = 131_072;
const RECORD_WORDS = QUAD_RECORD_BYTES / 4;
const digests = new WeakMap<TextureIdentity, Uint8Array>();

/**
 * A texture's 32 digest bytes, parsed once per identity object: reuse the
 * handle `uploadTexture` returns rather than building identities per quad.
 */
function digestBytes(texture: TextureIdentity): Uint8Array {
  let bytes = digests.get(texture);
  if (bytes === undefined) {
    const digest = texture.digest;
    if (typeof digest !== 'string' || !/^[0-9a-f]{64}$/.test(digest)) {
      throw new TilewrightError(
        'invalid-argument',
        'texture digest must be 64 lowercase hexadecimal digits',
      );
    }
    bytes = new Uint8Array(32);
    for (let index = 0; index < 32; index += 1) {
      bytes[index] = parseInt(digest.slice(index * 2, index * 2 + 2), 16);
    }
    digests.set(texture, bytes);
  }
  return bytes;
}
const FLAG_PICKABLE = 1;
const FLAG_LINEAR = 1 << 1;
const FLAG_GLOW = 1 << 2;
const FLAG_FRAME = 1 << 3;
const LAYER_SHIFT = 8;
const LAYERS: Record<Layer, number> = { world: 0, effect: 1, interface: 2 };
const WHOLE_UVS: readonly [Point, Point, Point, Point] = [
  [0, 0],
  [1, 0],
  [1, 1],
  [0, 1],
];
const WHITE: Color = [1, 1, 1, 1];
const LITTLE_ENDIAN = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1;

function integer(value: number, name: string, minimum: number, maximum: number): number {
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    throw new TilewrightError(
      'invalid-argument',
      `${name} must be an integer from ${minimum} to ${maximum}`,
    );
  }
  return value;
}

const U32_MAX = 0xffff_ffff;
const SAFE_MAX = Number.MAX_SAFE_INTEGER;

/** Builds one scene as packed quad records. */
export class SceneBuilder {
  #words: Uint32Array;
  #floats: Float32Array;
  #bytes: Uint8Array;
  #count = 0;
  #options: Required<SceneOptions>;

  /** `capacity` quads are allocated up front, at most `MAXIMUM_QUADS`. */
  constructor(options: SceneOptions, capacity = 1_024) {
    if (!LITTLE_ENDIAN) {
      throw new TilewrightError('unsupported', 'packed scenes need a little-endian host');
    }
    this.#options = SceneBuilder.#resolve(options);
    const quads = integer(capacity, 'capacity', 0, MAXIMUM_QUADS);
    const buffer = new ArrayBuffer(Math.max(1, quads) * QUAD_RECORD_BYTES);
    this.#words = new Uint32Array(buffer);
    this.#floats = new Float32Array(buffer);
    this.#bytes = new Uint8Array(buffer);
  }

  static #resolve(options: SceneOptions): Required<SceneOptions> {
    return {
      width: integer(options.width, 'width', 1, U32_MAX),
      height: integer(options.height, 'height', 1, U32_MAX),
      clearColor: options.clearColor ?? [0, 0, 0, 1],
      scaleMilli: integer(options.scaleMilli ?? 1_000, 'scaleMilli', 1, U32_MAX),
      revision: integer(options.revision ?? 1, 'revision', 1, SAFE_MAX),
      clockMillis: integer(options.clockMillis ?? 0, 'clockMillis', 0, SAFE_MAX),
    };
  }

  /** Empties the scene, keeping its buffer, and optionally changes its options. */
  reset(options: Partial<SceneOptions> = {}): this {
    this.#options = SceneBuilder.#resolve({ ...this.#options, ...options });
    this.#count = 0;
    return this;
  }

  /** The scene-wide options. */
  get options(): Readonly<Required<SceneOptions>> {
    return this.#options;
  }

  /** The number of quads added. */
  get length(): number {
    return this.#count;
  }

  /** The packed records of every quad added, without copying. */
  records(): Uint8Array {
    return new Uint8Array(this.#words.buffer, 0, this.#count * QUAD_RECORD_BYTES);
  }

  /** Adds an axis-aligned sprite. */
  sprite(sprite: SpriteInput): this {
    if (!(sprite.width > 0) || !(sprite.height > 0)) {
      throw new TilewrightError('invalid-argument', 'sprite width and height must be positive');
    }
    const left = sprite.x;
    const top = sprite.y;
    const right = sprite.x + sprite.width;
    const bottom = sprite.y + sprite.height;
    return this.quad({
      ...sprite,
      positions: [
        [left, top],
        [right, top],
        [right, bottom],
        [left, bottom],
      ],
    });
  }

  /** Adds a quad. Integer fields are checked here; the renderer validates the rest on submit. */
  quad(quad: QuadInput): this {
    // The renderer would refuse a larger scene; stop before allocating for it.
    if (this.#count >= MAXIMUM_QUADS) {
      throw new TilewrightError(
        'limit-exceeded',
        `a scene holds at most ${MAXIMUM_QUADS} quads`,
      );
    }
    this.#reserve(this.#count + 1);
    const words = this.#words;
    const floats = this.#floats;
    const base = this.#count * RECORD_WORDS;
    const part = integer(quad.part ?? 0, 'part', 0, 0xffff);
    const depth = integer(quad.depth ?? 0, 'depth', -0x8000, 0x7fff);
    const layer = LAYERS[quad.layer ?? 'world'];
    if (layer === undefined) {
      throw new TilewrightError('invalid-argument', `unknown layer ${String(quad.layer)}`);
    }
    const sampling = quad.sampling ?? 'nearest';
    if (sampling !== 'nearest' && sampling !== 'linear') {
      throw new TilewrightError('invalid-argument', `unknown sampling ${String(sampling)}`);
    }
    const pickable = quad.pickable ?? quad.glow === undefined;
    words[base] = integer(quad.id, 'id', 1, U32_MAX);
    words[base + 1] = (part | ((depth & 0xffff) << 16)) >>> 0;
    words[base + 2] = integer(quad.order ?? 0, 'order', 0, U32_MAX);
    words[base + 3] =
      (layer << LAYER_SHIFT) |
      (pickable ? FLAG_PICKABLE : 0) |
      (sampling === 'linear' ? FLAG_LINEAR : 0) |
      (quad.glow ? FLAG_GLOW : 0) |
      (quad.frame ? FLAG_FRAME : 0);
    const id = integer(quad.texture.id, 'texture id', 1, SAFE_MAX);
    const revision = integer(quad.texture.revision, 'texture revision', 1, SAFE_MAX);
    const digest = digestBytes(quad.texture);
    words[base + 4] = id >>> 0;
    words[base + 5] = Math.floor(id / 0x1_0000_0000);
    words[base + 6] = revision >>> 0;
    words[base + 7] = Math.floor(revision / 0x1_0000_0000);
    const frame = quad.frame;
    words[base + 8] = frame ? integer(frame.x, 'frame x', 0, U32_MAX) : 0;
    words[base + 9] = frame ? integer(frame.y, 'frame y', 0, U32_MAX) : 0;
    words[base + 10] = frame ? integer(frame.width, 'frame width', 1, U32_MAX) : 0;
    words[base + 11] = frame ? integer(frame.height, 'frame height', 1, U32_MAX) : 0;
    const uvs = quad.uvs ?? WHOLE_UVS;
    for (let corner = 0; corner < 4; corner += 1) {
      floats[base + 12 + corner * 2] = quad.positions[corner][0];
      floats[base + 13 + corner * 2] = quad.positions[corner][1];
      floats[base + 20 + corner * 2] = uvs[corner][0];
      floats[base + 21 + corner * 2] = uvs[corner][1];
    }
    const color = quad.color ?? WHITE;
    floats.set(color, base + 28);
    floats[base + 32] = quad.grayscale ?? 0;
    if (quad.glow) {
      floats.set(quad.glow.color, base + 33);
      floats[base + 37] = quad.glow.radius[0];
      floats[base + 38] = quad.glow.radius[1];
    } else {
      words.fill(0, base + 33, base + 39);
    }
    this.#bytes.set(digest, this.#count * QUAD_RECORD_BYTES + DIGEST_OFFSET);
    this.#count += 1;
    return this;
  }

  #reserve(count: number): void {
    if (count <= this.#words.length / RECORD_WORDS) {
      return;
    }
    const capacity = Math.min(
      MAXIMUM_QUADS,
      Math.max(count, (this.#words.length / RECORD_WORDS) * 2),
    );
    const buffer = new ArrayBuffer(capacity * QUAD_RECORD_BYTES);
    const words = new Uint32Array(buffer);
    words.set(this.#words.subarray(0, this.#count * RECORD_WORDS));
    this.#words = words;
    this.#floats = new Float32Array(buffer);
    this.#bytes = new Uint8Array(buffer);
  }
}

export interface SpriteSheetOptions {
  readonly frameWidth: number;
  readonly frameHeight: number;
  readonly columns: number;
  readonly rows: number;
  /** Top-left texel of frame 0. */
  readonly origin?: Point;
  /** Texels between neighbouring frames along each axis. */
  readonly spacing?: Point;
}

/**
 * A regular grid of equally sized frames in one texture, numbered row by row
 * from the top-left frame, starting at 0.
 */
export class SpriteSheet {
  readonly frameWidth: number;
  readonly frameHeight: number;
  readonly columns: number;
  readonly rows: number;
  readonly origin: Point;
  readonly spacing: Point;

  constructor(options: SpriteSheetOptions) {
    this.frameWidth = integer(options.frameWidth, 'frameWidth', 1, U32_MAX);
    this.frameHeight = integer(options.frameHeight, 'frameHeight', 1, U32_MAX);
    this.columns = integer(options.columns, 'columns', 0, U32_MAX);
    this.rows = integer(options.rows, 'rows', 0, U32_MAX);
    const origin = options.origin ?? [0, 0];
    const spacing = options.spacing ?? [0, 0];
    this.origin = [
      integer(origin[0], 'origin x', 0, U32_MAX),
      integer(origin[1], 'origin y', 0, U32_MAX),
    ];
    this.spacing = [
      integer(spacing[0], 'spacing x', 0, U32_MAX),
      integer(spacing[1], 'spacing y', 0, U32_MAX),
    ];
  }

  /** A gapless grid starting at the texture's top-left texel. */
  static grid(frameWidth: number, frameHeight: number, columns: number, rows: number): SpriteSheet {
    return new SpriteSheet({ frameWidth, frameHeight, columns, rows });
  }

  /** The gapless grid of whole frames that fits a texture, such as a character sheet. */
  static covering(
    texture: { readonly width: number; readonly height: number },
    frameWidth: number,
    frameHeight: number,
  ): SpriteSheet {
    return SpriteSheet.grid(
      frameWidth,
      frameHeight,
      Math.floor(texture.width / frameWidth),
      Math.floor(texture.height / frameHeight),
    );
  }

  /** The number of frames, or `undefined` if it does not fit in 32 bits. */
  get frameCount(): number | undefined {
    const count = this.columns * this.rows;
    return count <= U32_MAX ? count : undefined;
  }

  /**
   * The frame at a column and row. Like the Rust `SpriteSheet`, every
   * coordinate must fit in 32 bits; spacing lies only between frames.
   */
  frameAt(column: number, row: number): TextureRect {
    integer(column, 'column', 0, this.columns - 1);
    integer(row, 'row', 0, this.rows - 1);
    const offset = (origin: number, size: number, spacing: number, index: number): number => {
      const start = origin + size * index + spacing * index;
      if (size * index > U32_MAX || spacing * index > U32_MAX || start + size > U32_MAX) {
        throw new TilewrightError('invalid-argument', 'frame coordinates must fit in 32 bits');
      }
      return start;
    };
    return {
      x: offset(this.origin[0], this.frameWidth, this.spacing[0], column),
      y: offset(this.origin[1], this.frameHeight, this.spacing[1], row),
      width: this.frameWidth,
      height: this.frameHeight,
    };
  }

  /** Frame `index`, counting row by row. */
  frame(index: number): TextureRect {
    integer(index, 'frame index', 0, U32_MAX);
    if (this.columns === 0) {
      throw new TilewrightError('invalid-argument', 'the sheet has no frames');
    }
    return this.frameAt(index % this.columns, Math.floor(index / this.columns));
  }
}
