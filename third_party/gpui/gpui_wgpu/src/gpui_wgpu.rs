// Proving `wgpu::Device: Send` (clippy's `arc_with_non_send_sync` check on
// `Arc::new(device)`) walks the nested wgpu_core hub and registry types past
// the default depth of 128, and newer toolchains report that overflow as a
// warning, which `-D warnings` turns into a build failure.
#![recursion_limit = "256"]

mod cosmic_text_system;
mod wgpu_atlas;
mod wgpu_context;
mod wgpu_renderer;

pub use cosmic_text_system::*;
pub use wgpu;
pub use wgpu_atlas::*;
pub use wgpu_context::*;
pub use wgpu_renderer::{GpuContext, WgpuRenderer, WgpuSurfaceConfig};
