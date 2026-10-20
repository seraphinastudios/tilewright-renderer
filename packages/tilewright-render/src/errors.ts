// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

/**
 * Error codes:
 *
 * - `invalid-argument`: a value the API does not accept.
 * - `invalid-texture`: an upload with a bad identity, size or pixel length.
 * - `invalid-scene`: a scene the renderer's scene contract rejects.
 * - `unknown-texture`: a scene naming a texture that is not uploaded.
 * - `limit-exceeded`: a size, count or memory bound.
 * - `invalid-target`: no scene, or a scene not matching the canvas size.
 * - `backend-unavailable`: the browser offers no adapter for the backend.
 * - `surface-unavailable`: the browser skipped presenting this frame.
 * - `surface-lost`, `device-lost`: recreate the renderer.
 * - `resource`: a texture failed to load or a frame lies outside its texture.
 * - `busy`: a readback is still in flight.
 * - `destroyed`: the renderer was destroyed.
 * - `unsupported`: the host cannot run the package.
 * - `internal`: an unexpected renderer failure.
 */
export type TilewrightErrorCode =
  | 'invalid-argument'
  | 'invalid-texture'
  | 'invalid-scene'
  | 'unknown-texture'
  | 'limit-exceeded'
  | 'invalid-target'
  | 'backend-unavailable'
  | 'surface-unavailable'
  | 'surface-lost'
  | 'device-lost'
  | 'resource'
  | 'busy'
  | 'destroyed'
  | 'unsupported'
  | 'internal';

const CODES = new Set<string>([
  'invalid-argument',
  'invalid-texture',
  'invalid-scene',
  'unknown-texture',
  'limit-exceeded',
  'invalid-target',
  'backend-unavailable',
  'surface-unavailable',
  'surface-lost',
  'device-lost',
  'resource',
  'busy',
  'destroyed',
  'unsupported',
  'internal',
]);

/** A renderer error with a stable code. */
export class TilewrightError extends Error {
  readonly code: TilewrightErrorCode;

  constructor(code: TilewrightErrorCode, message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = 'TilewrightError';
    this.code = code;
  }

  /** Converts an error thrown by the WebAssembly bindings. */
  static from(error: unknown): TilewrightError {
    if (error instanceof TilewrightError) {
      return error;
    }
    const message = error instanceof Error ? error.message : String(error);
    const separator = message.indexOf(': ');
    const code = separator > 0 ? message.slice(0, separator) : '';
    if (CODES.has(code)) {
      return new TilewrightError(
        code as TilewrightErrorCode,
        message.slice(separator + 2),
        { cause: error },
      );
    }
    return new TilewrightError('internal', message, { cause: error });
  }
}
