// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Browser bindings for the Tilewright renderer: WebGPU, with WebGL2 where
//! WebGPU is unavailable, through `wasm-bindgen`. The TypeScript package in
//! `packages/tilewright-render` wraps these bindings; see `docs/WEB.md`.
//!
//! Pages upload decoded RGBA8 textures into a [`textures::WebTextures`]
//! store and submit scenes as [`packed`] quad records. The store and the
//! record decoder build on every target so they are tested natively; the
//! bindings themselves exist only on `wasm32`.

pub mod errors;
pub mod packed;
pub mod textures;

#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_arch = "wasm32")]
pub use web::{WebFrame, WebMetrics, WebRenderer};
