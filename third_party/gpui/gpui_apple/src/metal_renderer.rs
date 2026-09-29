use crate::metal_atlas::MetalAtlas;
use anyhow::{Context as _, Result};
use block2::RcBlock;
use gpui::{
    AtlasTextureId, BackdropBlur, Background, Bounds, ContentMask, Corners, DevicePixels,
    PaintSurface, Path, Point, PrimitiveBatch, ScaledPixels, Scene, Size, point, size,
};
#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
use image::RgbaImage;
use objc2_metal::MTLBlitCommandEncoder;
#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
use objc2_metal::{MTLOrigin, MTLRegion, MTLSize};

use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_core_foundation::CFRetained;
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer,
    CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPixelFormatType,
    CVPixelBufferGetWidth, CVPixelBufferGetWidthOfPlane,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, kCVReturnSuccess,
};
use objc2_foundation::{NSRange, NSSize, NSString};
use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLBuffer, MTLClearColor, MTLCommandBuffer,
    MTLCommandEncoder, MTLCommandQueue, MTLCopyAllDevices, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLDrawable, MTLGPUFamily, MTLLibrary, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
    MTLRenderPipelineState, MTLResourceOptions, MTLStorageMode, MTLStoreAction, MTLTexture,
    MTLTextureDescriptor, MTLTextureType, MTLTextureUsage, MTLViewport,
};
use objc2_quartz_core::{CAAutoresizingMask, CAMetalDrawable, CAMetalLayer};
use parking_lot::Mutex;

use std::{
    cell::Cell, ffi::c_void, mem, mem::MaybeUninit, ops::Range, ptr, ptr::NonNull, slice, sync::Arc,
};

// Exported to metal
pub(crate) type PointF = gpui::Point<f32>;

#[cfg(not(feature = "runtime_shaders"))]
const SHADERS_METALLIB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shaders.metallib"));
#[cfg(feature = "runtime_shaders")]
const SHADERS_SOURCE_FILE: &str = include_str!(concat!(env!("OUT_DIR"), "/stitched_shaders.metal"));
// Use 4x MSAA, all devices support it.
// https://developer.apple.com/documentation/metal/mtldevice/1433355-supportstexturesamplecount
const PATH_SAMPLE_COUNT: u32 = 4;
/// Metal requires the offset a buffer is bound at to be 256-byte aligned.
const INSTANCE_BUFFER_ALIGNMENT: usize = 256;
const MAX_INSTANCE_BUFFER_SIZE: usize = 256 * 1024 * 1024;
/// The backdrop is reduced before it is blurred. A Gaussian is scale invariant,
/// so shrinking the image and shrinking the kernel by the same factor gives the
/// same result for a sixteenth of the samples, and the difference the reduction
/// costs is detail that a blur destroys anyway.
const BACKDROP_BLUR_DOWNSCALE: usize = 4;
/// Variance of the box filter the reduction applies, `(n^2 - 1) / 12` for an
/// `n`-wide box. Subtracting it from the requested variance keeps a small radius
/// from coming out wider than asked.
const BACKDROP_BLUR_DOWNSCALE_VARIANCE: f32 =
    ((BACKDROP_BLUR_DOWNSCALE * BACKDROP_BLUR_DOWNSCALE) as f32 - 1.0) / 12.0;
/// Averaging gamma-encoded colors darkens them, so the reduced copies hold
/// decoded values and need more range and precision than 8-bit unorm.
const BACKDROP_BLUR_FORMAT: MTLPixelFormat = MTLPixelFormat::RGBA16Float;

pub type Context = Arc<Mutex<InstanceBufferPool>>;
pub type Renderer = MetalRenderer;

pub unsafe fn new_renderer(
    context: self::Context,
    _native_window: *mut c_void,
    _native_view: *mut c_void,
    _bounds: gpui::Size<f32>,
    transparent: bool,
) -> Renderer {
    MetalRenderer::new(context, transparent)
}

pub struct InstanceBufferPool {
    buffer_size: usize,
    buffers: Vec<Retained<ProtocolObject<dyn MTLBuffer>>>,
}

// The pool is shared with command buffer completion handlers, which Metal runs
// on its own thread to return buffers once the GPU is done with them. Metal
// buffer objects may be retained, released and handed between threads freely;
// objc2 just does not mark the protocol objects as `Send`.
unsafe impl Send for InstanceBufferPool {}

impl Default for InstanceBufferPool {
    fn default() -> Self {
        Self {
            buffer_size: 2 * 1024 * 1024,
            buffers: Vec::new(),
        }
    }
}

pub(crate) struct InstanceBuffer {
    metal_buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    size: usize,
}

impl InstanceBufferPool {
    pub(crate) fn reset(&mut self, buffer_size: usize) {
        self.buffer_size = buffer_size;
        self.buffers.clear();
    }

    pub(crate) fn acquire(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        unified_memory: bool,
    ) -> InstanceBuffer {
        let buffer = self.buffers.pop().unwrap_or_else(|| {
            let options = if unified_memory {
                MTLResourceOptions::StorageModeShared
                    // Buffers are write only which can benefit from the combined cache
                    // https://developer.apple.com/documentation/metal/mtlresourceoptions/cpucachemodewritecombined
                    | MTLResourceOptions::CPUCacheModeWriteCombined
            } else {
                MTLResourceOptions::StorageModeManaged
            };

            device
                .newBufferWithLength_options(self.buffer_size, options)
                .expect("failed to allocate instance buffer")
        });
        InstanceBuffer {
            metal_buffer: buffer,
            size: self.buffer_size,
        }
    }

    pub(crate) fn release(&mut self, buffer: InstanceBuffer) {
        if buffer.size == self.buffer_size {
            self.buffers.push(buffer.metal_buffer)
        }
    }
}

pub struct MetalRenderer {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    layer: Option<Retained<CAMetalLayer>>,
    is_apple_gpu: bool,
    is_unified_memory: bool,
    presents_with_transaction: bool,
    /// For headless rendering, tracks whether output should be opaque
    opaque: bool,
    command_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    paths_rasterization_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    path_sprites_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    shadows_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    quads_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    underlines_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    monochrome_sprites_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    polychrome_sprites_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    surfaces_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    backdrop_downsample_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    backdrop_blur_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    backdrop_composite_pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    /// Created on the first frame with a backdrop blur and whenever the frame
    /// size changes after that.
    backdrop_blur_targets: Option<BackdropBlurTargets>,
    unit_vertices: Retained<ProtocolObject<dyn MTLBuffer>>,
    #[allow(clippy::arc_with_non_send_sync)]
    instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
    sprite_atlas: Arc<MetalAtlas>,
    core_video_texture_cache: CFRetained<CVMetalTextureCache>,
    path_intermediate_texture: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    path_intermediate_msaa_texture: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    path_sample_count: u32,
    /// Offscreen render target reused across `render_scene` calls when
    /// rendering headlessly without reading pixels back.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    headless_render_target: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
}

#[repr(C)]
pub struct PathRasterizationVertex {
    pub xy_position: Point<ScaledPixels>,
    pub st_position: Point<f32>,
    pub color: Background,
    pub bounds: Bounds<ScaledPixels>,
}

impl MetalRenderer {
    /// Creates a new MetalRenderer with a CAMetalLayer for window-based rendering.
    pub fn new(instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>, transparent: bool) -> Self {
        let device = Self::create_device();

        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&device));
        layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        // Support direct-to-display rendering if the window is not transparent
        // https://developer.apple.com/documentation/metal/managing-your-game-window-for-metal-in-macos
        layer.setOpaque(!transparent);
        layer.setMaximumDrawableCount(3);
        // Allow texture reading for visual tests (captures screenshots without ScreenCaptureKit).
        // Other builds keep drawables framebuffer-only until a frame needs to
        // read one back for a backdrop blur; see `draw`.
        #[cfg(any(test, feature = "test-support"))]
        layer.setFramebufferOnly(false);
        layer.setAllowsNextDrawableTimeout(false);
        layer.setNeedsDisplayOnBoundsChange(true);
        layer.setAutoresizingMask(
            CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
        );

        Self::new_internal(device, Some(layer), !transparent, instance_buffer_pool)
    }

    /// Creates a new headless MetalRenderer for offscreen rendering without a window.
    ///
    /// This renderer can render scenes to images without requiring a CAMetalLayer,
    /// window, or AppKit. Use `render_scene_to_image()` to render scenes.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub fn new_headless(instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>) -> Self {
        let device = Self::create_device();
        Self::new_internal(device, None, true, instance_buffer_pool)
    }

    fn create_device() -> Retained<ProtocolObject<dyn MTLDevice>> {
        // Prefer low‐power integrated GPUs on Intel Mac. On Apple
        // Silicon, there is only ever one GPU, so this is equivalent to
        // `MTLCreateSystemDefaultDevice()`.
        if let Some(d) = MTLCopyAllDevices()
            .into_iter()
            .min_by_key(|d| (d.isRemovable(), !d.isLowPower()))
        {
            d
        } else {
            // For some reason `MTLCopyAllDevices()` can return an empty list, see https://github.com/zed-industries/zed/issues/37689
            // In that case, we fall back to the system default device.
            log::error!(
                "Unable to enumerate Metal devices; attempting to use system default device"
            );
            MTLCreateSystemDefaultDevice().unwrap_or_else(|| {
                log::error!("unable to access a compatible graphics device");
                std::process::exit(1);
            })
        }
    }

    fn new_internal(
        device: Retained<ProtocolObject<dyn MTLDevice>>,
        layer: Option<Retained<CAMetalLayer>>,
        opaque: bool,
        instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
    ) -> Self {
        #[cfg(feature = "runtime_shaders")]
        let library = device
            .newLibraryWithSource_options_error(
                &NSString::from_str(SHADERS_SOURCE_FILE),
                Some(&objc2_metal::MTLCompileOptions::new()),
            )
            .expect("error building metal library");
        #[cfg(not(feature = "runtime_shaders"))]
        let library = device
            .newLibraryWithData_error(&dispatch2::DispatchData::from_static_bytes(
                SHADERS_METALLIB,
            ))
            .expect("error building metal library");

        fn to_float2_bits(point: PointF) -> u64 {
            let mut output = point.y.to_bits() as u64;
            output <<= 32;
            output |= point.x.to_bits() as u64;
            output
        }

        // Shared memory can be used only if CPU and GPU share the same memory space.
        // https://developer.apple.com/documentation/metal/setting-resource-storage-modes
        let is_unified_memory = device.hasUnifiedMemory();
        // Apple GPU families support memoryless textures, which can significantly reduce
        // memory usage by keeping render targets in on-chip tile memory instead of
        // allocating backing store in system memory.
        // https://developer.apple.com/documentation/metal/mtlgpufamily
        let is_apple_gpu = device.supportsFamily(MTLGPUFamily::Apple1);

        let unit_vertices = [
            to_float2_bits(point(0., 0.)),
            to_float2_bits(point(1., 0.)),
            to_float2_bits(point(0., 1.)),
            to_float2_bits(point(0., 1.)),
            to_float2_bits(point(1., 0.)),
            to_float2_bits(point(1., 1.)),
        ];
        // Safety: the pointer and length describe the live `unit_vertices`
        // array, which Metal copies into the new buffer before returning.
        let unit_vertices = unsafe {
            device.newBufferWithBytes_length_options(
                NonNull::from(&unit_vertices).cast::<c_void>(),
                mem::size_of_val(&unit_vertices),
                if is_unified_memory {
                    MTLResourceOptions::StorageModeShared
                        | MTLResourceOptions::CPUCacheModeWriteCombined
                } else {
                    MTLResourceOptions::StorageModeManaged
                },
            )
        }
        .expect("failed to allocate unit vertex buffer");

        let paths_rasterization_pipeline_state = build_path_rasterization_pipeline_state(
            &device,
            &library,
            "paths_rasterization",
            "path_rasterization_vertex",
            "path_rasterization_fragment",
            MTLPixelFormat::BGRA8Unorm,
            PATH_SAMPLE_COUNT,
        );
        let path_sprites_pipeline_state = build_path_sprite_pipeline_state(
            &device,
            &library,
            "path_sprites",
            "path_sprite_vertex",
            "path_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let shadows_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "shadows",
            "shadow_vertex",
            "shadow_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let quads_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "quads",
            "quad_vertex",
            "quad_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let underlines_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "underlines",
            "underline_vertex",
            "underline_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let monochrome_sprites_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "monochrome_sprites",
            "monochrome_sprite_vertex",
            "monochrome_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let polychrome_sprites_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "polychrome_sprites",
            "polychrome_sprite_vertex",
            "polychrome_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let surfaces_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "surfaces",
            "surface_vertex",
            "surface_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );

        let backdrop_downsample_pipeline_state = build_backdrop_pass_pipeline_state(
            &device,
            &library,
            "backdrop_downsample",
            "backdrop_pass_vertex",
            "backdrop_downsample_fragment",
        );
        let backdrop_blur_pipeline_state = build_backdrop_pass_pipeline_state(
            &device,
            &library,
            "backdrop_blur",
            "backdrop_pass_vertex",
            "backdrop_blur_fragment",
        );
        let backdrop_composite_pipeline_state = build_backdrop_composite_pipeline_state(
            &device,
            &library,
            "backdrop_composite",
            "backdrop_composite_vertex",
            "backdrop_composite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );

        let command_queue = device
            .newCommandQueue()
            .expect("failed to create metal command queue");
        let sprite_atlas = Arc::new(MetalAtlas::new(device.clone(), is_apple_gpu));
        let core_video_texture_cache = new_core_video_texture_cache(&device);

        Self {
            device,
            layer,
            presents_with_transaction: false,
            is_apple_gpu,
            is_unified_memory,
            opaque,
            command_queue,
            paths_rasterization_pipeline_state,
            path_sprites_pipeline_state,
            shadows_pipeline_state,
            quads_pipeline_state,
            underlines_pipeline_state,
            monochrome_sprites_pipeline_state,
            polychrome_sprites_pipeline_state,
            surfaces_pipeline_state,
            backdrop_downsample_pipeline_state,
            backdrop_blur_pipeline_state,
            backdrop_composite_pipeline_state,
            backdrop_blur_targets: None,
            unit_vertices,
            instance_buffer_pool,
            sprite_atlas,
            core_video_texture_cache,
            path_intermediate_texture: None,
            path_intermediate_msaa_texture: None,
            path_sample_count: PATH_SAMPLE_COUNT,
            #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
            headless_render_target: None,
        }
    }

    pub fn layer(&self) -> Option<&CAMetalLayer> {
        self.layer.as_deref()
    }

    pub fn layer_ptr(&self) -> *mut CAMetalLayer {
        self.layer
            .as_ref()
            .map(|l| Retained::as_ptr(l).cast_mut())
            .unwrap_or(ptr::null_mut())
    }

    pub fn sprite_atlas(&self) -> &Arc<MetalAtlas> {
        &self.sprite_atlas
    }

    pub fn set_presents_with_transaction(&mut self, presents_with_transaction: bool) {
        self.presents_with_transaction = presents_with_transaction;
        if let Some(layer) = &self.layer {
            layer.setPresentsWithTransaction(presents_with_transaction);
        }
    }

    pub fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        if let Some(layer) = &self.layer {
            let ns_size = NSSize::new(size.width.0 as f64, size.height.0 as f64);
            layer.setDrawableSize(ns_size);
        }
        self.update_path_intermediate_textures(size);
    }

    fn update_path_intermediate_textures(&mut self, size: Size<DevicePixels>) {
        // We are uncertain when this happens, but sometimes size can be 0 here. Most likely before
        // the layout pass on window creation. Zero-sized texture creation causes SIGABRT.
        // https://github.com/zed-industries/zed/issues/36229
        if size.width.0 <= 0 || size.height.0 <= 0 {
            self.path_intermediate_texture = None;
            self.path_intermediate_msaa_texture = None;
            return;
        }

        let texture_descriptor = MTLTextureDescriptor::new();
        // Safety: both dimensions were checked to be positive above.
        unsafe {
            texture_descriptor.setWidth(size.width.0 as usize);
            texture_descriptor.setHeight(size.height.0 as usize);
        }
        texture_descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        texture_descriptor.setStorageMode(MTLStorageMode::Private);
        texture_descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        self.path_intermediate_texture = Some(new_texture(&self.device, &texture_descriptor));

        if self.path_sample_count > 1 {
            // https://developer.apple.com/documentation/metal/choosing-a-resource-storage-mode-for-apple-gpus
            // Rendering MSAA textures are done in a single pass, so we can use memory-less storage on Apple Silicon
            let storage_mode = if self.is_apple_gpu {
                MTLStorageMode::Memoryless
            } else {
                MTLStorageMode::Private
            };

            let msaa_descriptor = texture_descriptor;
            msaa_descriptor.setTextureType(MTLTextureType::Type2DMultisample);
            msaa_descriptor.setStorageMode(storage_mode);
            // Safety: every Metal device supports 4x MSAA, the only sample
            // count this renderer uses.
            unsafe { msaa_descriptor.setSampleCount(self.path_sample_count as usize) };
            self.path_intermediate_msaa_texture = Some(new_texture(&self.device, &msaa_descriptor));
        } else {
            self.path_intermediate_msaa_texture = None;
        }
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        self.opaque = !transparent;
        if let Some(layer) = &self.layer {
            layer.setOpaque(!transparent);
        }
    }

    pub fn destroy(&self) {
        // nothing to do
    }

    pub fn draw(&mut self, scene: &Scene) {
        let layer = match &self.layer {
            Some(l) => l.clone(),
            None => {
                log::error!(
                    "draw() called on headless renderer - use render_scene_to_image() instead"
                );
                return;
            }
        };
        let viewport_size = layer.drawableSize();
        let viewport_size: Size<DevicePixels> = size(
            (viewport_size.width.ceil() as i32).into(),
            (viewport_size.height.ceil() as i32).into(),
        );
        // A backdrop blur copies what the frame has painted so far out of the
        // drawable, which a framebuffer-only drawable forbids. Frames without
        // one keep the framebuffer-only drawables the display path is
        // optimized for; the property only changes when a blur appears or
        // goes away.
        #[cfg(not(any(test, feature = "test-support")))]
        {
            let framebuffer_only = scene.backdrop_blurs.is_empty();
            if layer.framebufferOnly() != framebuffer_only {
                layer.setFramebufferOnly(framebuffer_only);
            }
        }
        let drawable = if let Some(drawable) = layer.nextDrawable() {
            drawable
        } else {
            log::error!(
                "failed to retrieve next drawable, drawable size: {:?}",
                viewport_size
            );
            return;
        };

        let command_buffer = match self.render_frame(scene, &drawable.texture(), viewport_size) {
            Ok(command_buffer) => command_buffer,
            Err(error) => {
                log::error!("failed to render: {error:#}");
                return;
            }
        };

        if self.presents_with_transaction {
            command_buffer.commit();
            command_buffer.waitUntilScheduled();
            drawable.present();
        } else {
            command_buffer.presentDrawable(ProtocolObject::from_ref(&*drawable));
            command_buffer.commit();
        }
    }

    fn render_frame(
        &mut self,
        scene: &Scene,
        texture: &ProtocolObject<dyn MTLTexture>,
        viewport_size: Size<DevicePixels>,
    ) -> Result<Retained<ProtocolObject<dyn MTLCommandBuffer>>> {
        let mut writer = InstanceBufferWriter::new(
            &self.device,
            &self.instance_buffer_pool,
            self.is_unified_memory,
        );
        let instance_bindings = write_instances(scene, &mut writer).with_context(|| {
            format!(
                "scene too large: {} paths, {} shadows, {} quads, {} underlines, {} mono, {} poly, {} surfaces",
                scene.paths.len(),
                scene.shadows.len(),
                scene.quads.len(),
                scene.underlines.len(),
                scene.monochrome_sprites.len(),
                scene.polychrome_sprites.len(),
                scene.surfaces.len(),
            )
        })?;
        let command_buffer = self.draw_primitives_to_texture(
            scene,
            &instance_bindings,
            &mut writer,
            texture,
            viewport_size,
        )?;

        let instance_buffer_pool = self.instance_buffer_pool.clone();
        let instance_buffer = Cell::new(Some(writer.finish()));
        let block = RcBlock::new(move |_| {
            if let Some(instance_buffer) = instance_buffer.take() {
                instance_buffer_pool.lock().release(instance_buffer);
            }
        });
        // Safety: the block is a valid heap block, which Metal copies and
        // keeps alive until the command buffer completes.
        unsafe { command_buffer.addCompletedHandler(RcBlock::as_ptr(&block)) };

        Ok(command_buffer)
    }

    /// Renders the scene to a texture and returns the pixel data as an RGBA image.
    /// This does not present the frame to screen - useful for visual testing
    /// where we want to capture what would be rendered without displaying it.
    ///
    /// Note: This requires a layer-backed renderer. For headless rendering,
    /// use `render_scene_to_image()` instead.
    #[cfg(any(test, feature = "test-support"))]
    pub fn render_to_image(&mut self, scene: &Scene) -> Result<RgbaImage> {
        let layer = self
            .layer
            .clone()
            .ok_or_else(|| anyhow::anyhow!("render_to_image requires a layer-backed renderer"))?;
        let viewport_size = layer.drawableSize();
        let viewport_size: Size<DevicePixels> = size(
            (viewport_size.width.ceil() as i32).into(),
            (viewport_size.height.ceil() as i32).into(),
        );
        let drawable = layer
            .nextDrawable()
            .ok_or_else(|| anyhow::anyhow!("Failed to get drawable for render_to_image"))?;

        let command_buffer = self.render_frame(scene, &drawable.texture(), viewport_size)?;

        // Commit and wait for completion without presenting
        command_buffer.commit();
        command_buffer.waitUntilCompleted();

        read_texture_to_image(&drawable.texture())
    }

    /// Renders a scene to an image without requiring a window or CAMetalLayer.
    ///
    /// This is the primary method for headless rendering. It creates an offscreen
    /// texture, renders the scene to it, and returns the pixel data as an RGBA image.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> Result<RgbaImage> {
        if size.width.0 <= 0 || size.height.0 <= 0 {
            anyhow::bail!("Invalid size for render_scene_to_image: {:?}", size);
        }

        // Update path intermediate textures for this size
        self.update_path_intermediate_textures(size);

        // Create an offscreen texture as render target
        let texture_descriptor = MTLTextureDescriptor::new();
        // Safety: both dimensions were checked to be positive above.
        unsafe {
            texture_descriptor.setWidth(size.width.0 as usize);
            texture_descriptor.setHeight(size.height.0 as usize);
        }
        texture_descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        texture_descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        texture_descriptor.setStorageMode(MTLStorageMode::Managed);
        let target_texture = new_texture(&self.device, &texture_descriptor);

        let command_buffer = self.render_frame(scene, &target_texture, size)?;

        // On discrete GPUs (non-unified memory), Managed textures require an
        // explicit blit synchronize before the CPU can read back the rendered
        // data. Without this, get_bytes returns stale zeros.
        if !self.is_unified_memory {
            let blit = command_buffer
                .blitCommandEncoder()
                .expect("failed to create blit command encoder");
            blit.synchronizeResource(ProtocolObject::from_ref(&*target_texture));
            blit.endEncoding();
        }

        // Commit and wait for completion
        command_buffer.commit();
        command_buffer.waitUntilCompleted();

        read_texture_to_image(&target_texture)
    }

    /// Renders a scene to a reused offscreen texture without reading pixels
    /// back or blocking on GPU completion.
    ///
    /// This mirrors the CPU cost of presenting a frame to a window (scene
    /// encoding, instance buffer writes, command submission) and is used by
    /// headless benchmark rendering, where the produced pixels are never
    /// inspected.
    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> Result<()> {
        if size.width.0 <= 0 || size.height.0 <= 0 {
            anyhow::bail!("Invalid size for render_scene: {:?}", size);
        }

        self.update_path_intermediate_textures(size);

        let needs_new_target = self.headless_render_target.as_ref().is_none_or(|texture| {
            texture.width() != size.width.0 as usize || texture.height() != size.height.0 as usize
        });
        if needs_new_target {
            let texture_descriptor = MTLTextureDescriptor::new();
            // Safety: both dimensions were checked to be positive above.
            unsafe {
                texture_descriptor.setWidth(size.width.0 as usize);
                texture_descriptor.setHeight(size.height.0 as usize);
            }
            texture_descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            texture_descriptor
                .setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
            texture_descriptor.setStorageMode(MTLStorageMode::Private);
            self.headless_render_target = Some(new_texture(&self.device, &texture_descriptor));
        }
        let target_texture = self
            .headless_render_target
            .clone()
            .expect("just ensured the render target exists");

        let command_buffer = self.render_frame(scene, &target_texture, size)?;

        // Commit without waiting, mirroring presentation to a real window where
        // the CPU doesn't block on the GPU.
        command_buffer.commit();
        Ok(())
    }

    fn draw_primitives_to_texture(
        &mut self,
        scene: &Scene,
        instance_bindings: &InstanceBindings,
        writer: &mut InstanceBufferWriter,
        texture: &ProtocolObject<dyn MTLTexture>,
        viewport_size: Size<DevicePixels>,
    ) -> Result<Retained<ProtocolObject<dyn MTLCommandBuffer>>> {
        let command_queue = self.command_queue.clone();
        let command_buffer = command_queue
            .commandBuffer()
            .context("failed to create metal command buffer")?;
        let alpha = if self.opaque { 1. } else { 0. };

        let mut command_encoder = new_command_encoder_for_texture(
            &command_buffer,
            texture,
            viewport_size,
            Some(MTLClearColor {
                red: 0.,
                green: 0.,
                blue: 0.,
                alpha,
            }),
        );

        for batch in scene.batches() {
            match batch {
                PrimitiveBatch::Shadows(range) => {
                    self.draw_shadows(range, instance_bindings, viewport_size, &command_encoder)
                }
                PrimitiveBatch::BackdropBlurs(range) => {
                    command_encoder.endEncoding();

                    self.draw_backdrop_blurs(
                        &scene.backdrop_blurs[range],
                        texture,
                        viewport_size,
                        &command_buffer,
                    )?;

                    command_encoder = new_command_encoder_for_texture(
                        &command_buffer,
                        texture,
                        viewport_size,
                        None,
                    );
                }
                PrimitiveBatch::Quads(range) => {
                    self.draw_quads(range, instance_bindings, viewport_size, &command_encoder)
                }
                PrimitiveBatch::Paths(range) => {
                    let paths = &scene.paths[range];
                    command_encoder.endEncoding();

                    let did_draw = self.draw_paths_to_intermediate(
                        paths,
                        writer,
                        viewport_size,
                        &command_buffer,
                    )?;

                    command_encoder = new_command_encoder_for_texture(
                        &command_buffer,
                        texture,
                        viewport_size,
                        None,
                    );

                    if did_draw {
                        if let Err(error) = self.draw_paths_from_intermediate(
                            paths,
                            writer,
                            viewport_size,
                            &command_encoder,
                        ) {
                            command_encoder.endEncoding();
                            return Err(error);
                        }
                    }
                }
                PrimitiveBatch::Underlines(range) => {
                    self.draw_underlines(range, instance_bindings, viewport_size, &command_encoder)
                }
                PrimitiveBatch::MonochromeSprites { texture_id, range } => self
                    .draw_monochrome_sprites(
                        texture_id,
                        range,
                        instance_bindings,
                        viewport_size,
                        &command_encoder,
                    ),
                PrimitiveBatch::PolychromeSprites { texture_id, range } => self
                    .draw_polychrome_sprites(
                        texture_id,
                        range,
                        instance_bindings,
                        viewport_size,
                        &command_encoder,
                    ),
                PrimitiveBatch::Surfaces(range) => self.draw_surfaces(
                    &scene.surfaces[range.clone()],
                    range.start,
                    instance_bindings,
                    viewport_size,
                    &command_encoder,
                ),
                PrimitiveBatch::SubpixelSprites { .. } => unreachable!(),
            }
        }

        command_encoder.endEncoding();

        Ok(command_buffer)
    }

    /// Replace each region's backdrop with a blurred copy of itself: take a
    /// snapshot of what the frame has painted so far, reduce it, run the two
    /// separable Gaussian passes over the reduction, then upsample it back into
    /// the region's rounded shape.
    ///
    /// Every blur in the batch reads the same snapshot, so overlapping regions of
    /// one batch cannot smear each other.
    fn draw_backdrop_blurs(
        &mut self,
        blurs: &[BackdropBlur],
        texture: &ProtocolObject<dyn MTLTexture>,
        viewport_size: Size<DevicePixels>,
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
    ) -> Result<()> {
        if blurs.is_empty() || viewport_size.width.0 <= 0 || viewport_size.height.0 <= 0 {
            return Ok(());
        }

        if self
            .backdrop_blur_targets
            .as_ref()
            .is_none_or(|targets| targets.size != viewport_size)
        {
            self.backdrop_blur_targets =
                Some(BackdropBlurTargets::new(&self.device, viewport_size));
        }

        let targets = self
            .backdrop_blur_targets
            .as_ref()
            .context("backdrop blur targets missing")?;

        let blit = command_buffer
            .blitCommandEncoder()
            .context("failed to create backdrop snapshot encoder")?;
        // Safety: both textures have the frame's size and pixel format, and the
        // destination is not in use by any other encoder of this command buffer.
        unsafe { blit.copyFromTexture_toTexture(texture, &targets.source) };
        blit.endEncoding();

        let width = viewport_size.width.0 as f32;
        let height = viewport_size.height.0 as f32;
        let reduced_size = targets.reduced_size;
        let reduced_bounds = Bounds {
            origin: point(ScaledPixels(0.0), ScaledPixels(0.0)),
            size: size(ScaledPixels(reduced_size[0]), ScaledPixels(reduced_size[1])),
        };
        let reduced_max = [reduced_size[0] - 0.5, reduced_size[1] - 0.5];
        let scale = BACKDROP_BLUR_DOWNSCALE as f32;

        for blur in blurs {
            // The reduction already low-passed the image; asking the kernel for
            // the full requested width on top of it would over-blur.
            let sigma = (blur.sigma.0 * blur.sigma.0 - BACKDROP_BLUR_DOWNSCALE_VARIANCE)
                .max(0.0)
                .sqrt()
                / scale;
            // The reduction reads only the region itself, edge-replicating it
            // outward: the surrounding frame may be more transparent, and its
            // premultiplied color would darken the region's rim. Every later
            // pass covers the whole reduced image, which the reduction has
            // already filled with the region's edge outside the region.
            let region_min = [
                blur.bounds.origin.x.0.clamp(0.5, width - 0.5),
                blur.bounds.origin.y.0.clamp(0.5, height - 0.5),
            ];
            let region_max = [
                (blur.bounds.origin.x.0 + blur.bounds.size.width.0)
                    .clamp(region_min[0], width - 0.5),
                (blur.bounds.origin.y.0 + blur.bounds.size.height.0)
                    .clamp(region_min[1], height - 0.5),
            ];
            let pass = |direction, sigma, source_size, source_scale, source_min, source_max| {
                BackdropBlurPass {
                    bounds: reduced_bounds,
                    target_size: reduced_size,
                    source_size,
                    direction,
                    sigma,
                    source_scale,
                    source_min,
                    source_max,
                }
            };
            let passes = [
                // Reduction: no kernel, four taps per output pixel.
                pass(
                    [0.0, 0.0],
                    0.0,
                    [width, height],
                    scale,
                    region_min,
                    region_max,
                ),
                pass(
                    [1.0, 0.0],
                    sigma,
                    reduced_size,
                    1.0,
                    [0.5, 0.5],
                    reduced_max,
                ),
                pass(
                    [0.0, 1.0],
                    sigma,
                    reduced_size,
                    1.0,
                    [0.5, 0.5],
                    reduced_max,
                ),
            ];

            for (index, pass_params) in passes.iter().enumerate() {
                let (pipeline_state, source) = if index == 0 {
                    (&self.backdrop_downsample_pipeline_state, &targets.source)
                } else {
                    (
                        &self.backdrop_blur_pipeline_state,
                        &targets.scratch[(index - 1) % 2],
                    )
                };

                let render_pass_descriptor = MTLRenderPassDescriptor::new();
                // Safety: a render pass descriptor always provides color attachment 0.
                let color_attachment = unsafe {
                    render_pass_descriptor
                        .colorAttachments()
                        .objectAtIndexedSubscript(0)
                };
                color_attachment.setTexture(Some(&targets.scratch[index % 2]));
                // Every pass writes the whole reduced image.
                color_attachment.setLoadAction(MTLLoadAction::DontCare);
                color_attachment.setStoreAction(MTLStoreAction::Store);

                let command_encoder = command_buffer
                    .renderCommandEncoderWithDescriptor(&render_pass_descriptor)
                    .context("failed to create backdrop blur encoder")?;
                command_encoder.setRenderPipelineState(pipeline_state);
                // Safety: the parameters are copied into the command buffer
                // before this returns, the indices match the shader argument
                // tables, and the source texture lives in `self`.
                unsafe {
                    command_encoder.setVertexBuffer_offset_atIndex(
                        Some(&self.unit_vertices),
                        0,
                        BackdropBlurInputIndex::Vertices as usize,
                    );
                    command_encoder.setVertexBytes_length_atIndex(
                        NonNull::from(pass_params).cast(),
                        mem::size_of_val(pass_params),
                        BackdropBlurInputIndex::Params as usize,
                    );
                    command_encoder.setFragmentBytes_length_atIndex(
                        NonNull::from(pass_params).cast(),
                        mem::size_of_val(pass_params),
                        BackdropBlurInputIndex::Params as usize,
                    );
                    command_encoder.setFragmentTexture_atIndex(
                        Some(source),
                        BackdropBlurInputIndex::Source as usize,
                    );
                    command_encoder.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        6,
                    );
                }
                command_encoder.endEncoding();
            }

            let sprite = BackdropBlurSprite {
                bounds: blur.bounds,
                content_mask: blur.content_mask,
                corner_radii: blur.corner_radii,
                source_size: reduced_size,
                source_scale: scale,
                opacity: blur.opacity,
            };
            let blurred = &targets.scratch[(passes.len() - 1) % 2];
            let command_encoder =
                new_command_encoder_for_texture(command_buffer, texture, viewport_size, None);
            command_encoder.setRenderPipelineState(&self.backdrop_composite_pipeline_state);
            // Safety: as for the passes above; the viewport size is copied too.
            unsafe {
                command_encoder.setVertexBuffer_offset_atIndex(
                    Some(&self.unit_vertices),
                    0,
                    BackdropBlurInputIndex::Vertices as usize,
                );
                command_encoder.setVertexBytes_length_atIndex(
                    NonNull::from(&sprite).cast(),
                    mem::size_of_val(&sprite),
                    BackdropBlurInputIndex::Params as usize,
                );
                command_encoder.setVertexBytes_length_atIndex(
                    NonNull::from(&viewport_size).cast(),
                    mem::size_of_val(&viewport_size),
                    BackdropBlurInputIndex::ViewportSize as usize,
                );
                command_encoder.setFragmentBytes_length_atIndex(
                    NonNull::from(&sprite).cast(),
                    mem::size_of_val(&sprite),
                    BackdropBlurInputIndex::Params as usize,
                );
                command_encoder.setFragmentTexture_atIndex(
                    Some(blurred),
                    BackdropBlurInputIndex::Source as usize,
                );
                command_encoder.drawPrimitives_vertexStart_vertexCount(
                    MTLPrimitiveType::Triangle,
                    0,
                    6,
                );
            }
            command_encoder.endEncoding();
        }

        Ok(())
    }

    fn draw_paths_to_intermediate(
        &self,
        paths: &[Path<ScaledPixels>],
        writer: &mut InstanceBufferWriter,
        viewport_size: Size<DevicePixels>,
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
    ) -> Result<bool> {
        if paths.is_empty() {
            return Ok(false);
        }
        let intermediate_texture = self
            .path_intermediate_texture
            .as_ref()
            .context("missing path intermediate texture")?;

        let mut vertices = Vec::new();
        for path in paths {
            vertices.extend(path.vertices.iter().map(|v| PathRasterizationVertex {
                xy_position: v.xy_position,
                st_position: v.st_position,
                color: path.color,
                bounds: path.bounds.intersect(&path.content_mask.bounds),
            }));
        }
        let vertex_instance_bindings = writer.write(&vertices)?;

        let render_pass_descriptor = MTLRenderPassDescriptor::new();
        // Safety: a render pass descriptor always provides color attachment 0.
        let color_attachment = unsafe {
            render_pass_descriptor
                .colorAttachments()
                .objectAtIndexedSubscript(0)
        };
        color_attachment.setLoadAction(MTLLoadAction::Clear);
        color_attachment.setClearColor(MTLClearColor {
            red: 0.,
            green: 0.,
            blue: 0.,
            alpha: 0.,
        });

        if let Some(msaa_texture) = &self.path_intermediate_msaa_texture {
            color_attachment.setTexture(Some(msaa_texture));
            color_attachment.setResolveTexture(Some(intermediate_texture));
            color_attachment.setStoreAction(MTLStoreAction::MultisampleResolve);
        } else {
            color_attachment.setTexture(Some(intermediate_texture));
            color_attachment.setStoreAction(MTLStoreAction::Store);
        }

        let command_encoder = command_buffer
            .renderCommandEncoderWithDescriptor(&render_pass_descriptor)
            .context("failed to create path rasterization command encoder")?;
        command_encoder.setRenderPipelineState(&self.paths_rasterization_pipeline_state);
        // Safety: the vertex range lies inside a buffer written for this
        // frame, the indices match the shader argument table, and the vertex
        // count matches the number of vertices written.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&vertex_instance_bindings.buffer),
                vertex_instance_bindings.offset,
                PathRasterizationInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                PathRasterizationInputIndex::ViewportSize as usize,
            );
            command_encoder.setFragmentBuffer_offset_atIndex(
                Some(&vertex_instance_bindings.buffer),
                vertex_instance_bindings.offset,
                PathRasterizationInputIndex::Vertices as usize,
            );
            command_encoder.drawPrimitives_vertexStart_vertexCount(
                MTLPrimitiveType::Triangle,
                0,
                vertices.len(),
            );
        }

        command_encoder.endEncoding();
        Ok(true)
    }

    fn draw_shadows(
        &self,
        shadows: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) {
        if shadows.is_empty() {
            return;
        }

        command_encoder.setRenderPipelineState(&self.shadows_pipeline_state);
        // Safety: every bound buffer range lies inside a buffer written for
        // this frame, the indices match the shader argument tables, and the
        // draw counts match the number of instances written.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                ShadowInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&instance_bindings.shadows.buffer),
                instance_bindings.shadows.offset,
                ShadowInputIndex::Shadows as usize,
            );
            command_encoder.setFragmentBuffer_offset_atIndex(
                Some(&instance_bindings.shadows.buffer),
                instance_bindings.shadows.offset,
                ShadowInputIndex::Shadows as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                ShadowInputIndex::ViewportSize as usize,
            );

            command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                MTLPrimitiveType::Triangle,
                0,
                6,
                shadows.len(),
                shadows.start,
            );
        }
    }

    fn draw_quads(
        &self,
        quads: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) {
        if quads.is_empty() {
            return;
        }

        command_encoder.setRenderPipelineState(&self.quads_pipeline_state);
        // Safety: every bound buffer range lies inside a buffer written for
        // this frame, the indices match the shader argument tables, and the
        // draw counts match the number of instances written.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                QuadInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&instance_bindings.quads.buffer),
                instance_bindings.quads.offset,
                QuadInputIndex::Quads as usize,
            );
            command_encoder.setFragmentBuffer_offset_atIndex(
                Some(&instance_bindings.quads.buffer),
                instance_bindings.quads.offset,
                QuadInputIndex::Quads as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                QuadInputIndex::ViewportSize as usize,
            );

            command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                MTLPrimitiveType::Triangle,
                0,
                6,
                quads.len(),
                quads.start,
            );
        }
    }

    fn draw_paths_from_intermediate(
        &self,
        paths: &[Path<ScaledPixels>],
        writer: &mut InstanceBufferWriter,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) -> Result<()> {
        let Some(first_path) = paths.first() else {
            return Ok(());
        };
        let intermediate_texture = self
            .path_intermediate_texture
            .as_ref()
            .context("missing path intermediate texture")?;

        command_encoder.setRenderPipelineState(&self.path_sprites_pipeline_state);
        // Safety: every bound buffer range lies inside a buffer written for
        // this frame, the indices match the shader argument tables, and the
        // draw counts match the number of instances written.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                SpriteInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                SpriteInputIndex::ViewportSize as usize,
            );

            command_encoder.setFragmentTexture_atIndex(
                Some(intermediate_texture),
                SpriteInputIndex::AtlasTexture as usize,
            );
        }

        // When copying paths from the intermediate texture to the drawable,
        // each pixel must only be copied once, in case of transparent paths.
        //
        // If all paths have the same draw order, then their bounds are all
        // disjoint, so we can copy each path's bounds individually. If this
        // batch combines different draw orders, we perform a single copy
        // for a minimal spanning rect.
        let sprites;
        if paths.last().unwrap().order == first_path.order {
            sprites = paths
                .iter()
                .map(|path| PathSprite {
                    bounds: path.clipped_bounds(),
                })
                .collect();
        } else {
            let mut bounds = first_path.clipped_bounds();
            for path in paths.iter().skip(1) {
                bounds = bounds.union(&path.clipped_bounds());
            }
            sprites = vec![PathSprite { bounds }];
        }

        let sprite_instance_bindings = writer.write(&sprites)?;
        // Safety: every bound buffer range lies inside a buffer written for
        // this frame, the indices match the shader argument tables, and the
        // draw counts match the number of instances written.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&sprite_instance_bindings.buffer),
                sprite_instance_bindings.offset,
                SpriteInputIndex::Sprites as usize,
            );

            command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(
                MTLPrimitiveType::Triangle,
                0,
                6,
                sprites.len(),
            );
        }
        Ok(())
    }

    fn draw_underlines(
        &self,
        underlines: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) {
        if underlines.is_empty() {
            return;
        }

        command_encoder.setRenderPipelineState(&self.underlines_pipeline_state);
        // Safety: every bound buffer range lies inside a buffer written for
        // this frame, the indices match the shader argument tables, and the
        // draw counts match the number of instances written.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                UnderlineInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&instance_bindings.underlines.buffer),
                instance_bindings.underlines.offset,
                UnderlineInputIndex::Underlines as usize,
            );
            command_encoder.setFragmentBuffer_offset_atIndex(
                Some(&instance_bindings.underlines.buffer),
                instance_bindings.underlines.offset,
                UnderlineInputIndex::Underlines as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                UnderlineInputIndex::ViewportSize as usize,
            );

            command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                MTLPrimitiveType::Triangle,
                0,
                6,
                underlines.len(),
                underlines.start,
            );
        }
    }

    fn draw_monochrome_sprites(
        &self,
        texture_id: AtlasTextureId,
        sprites: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) {
        if sprites.is_empty() {
            return;
        }

        let texture = self.sprite_atlas.metal_texture(texture_id);
        let texture_size = size(
            DevicePixels(texture.width() as i32),
            DevicePixels(texture.height() as i32),
        );
        command_encoder.setRenderPipelineState(&self.monochrome_sprites_pipeline_state);
        // Safety: every bound buffer range lies inside a buffer written for
        // this frame, the indices match the shader argument tables, and the
        // draw counts match the number of instances written.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                SpriteInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&instance_bindings.monochrome_sprites.buffer),
                instance_bindings.monochrome_sprites.offset,
                SpriteInputIndex::Sprites as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                SpriteInputIndex::ViewportSize as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                NonNull::from(&texture_size).cast(),
                mem::size_of_val(&texture_size),
                SpriteInputIndex::AtlasTextureSize as usize,
            );
            command_encoder.setFragmentBuffer_offset_atIndex(
                Some(&instance_bindings.monochrome_sprites.buffer),
                instance_bindings.monochrome_sprites.offset,
                SpriteInputIndex::Sprites as usize,
            );
            command_encoder.setFragmentTexture_atIndex(
                Some(&texture),
                SpriteInputIndex::AtlasTexture as usize,
            );

            command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                MTLPrimitiveType::Triangle,
                0,
                6,
                sprites.len(),
                sprites.start,
            );
        }
    }

    fn draw_polychrome_sprites(
        &self,
        texture_id: AtlasTextureId,
        sprites: Range<usize>,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) {
        if sprites.is_empty() {
            return;
        }

        let texture = self.sprite_atlas.metal_texture(texture_id);
        let texture_size = size(
            DevicePixels(texture.width() as i32),
            DevicePixels(texture.height() as i32),
        );
        command_encoder.setRenderPipelineState(&self.polychrome_sprites_pipeline_state);
        // Safety: every bound buffer range lies inside a buffer written for
        // this frame, the indices match the shader argument tables, and the
        // draw counts match the number of instances written.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                SpriteInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&instance_bindings.polychrome_sprites.buffer),
                instance_bindings.polychrome_sprites.offset,
                SpriteInputIndex::Sprites as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                SpriteInputIndex::ViewportSize as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                NonNull::from(&texture_size).cast(),
                mem::size_of_val(&texture_size),
                SpriteInputIndex::AtlasTextureSize as usize,
            );
            command_encoder.setFragmentBuffer_offset_atIndex(
                Some(&instance_bindings.polychrome_sprites.buffer),
                instance_bindings.polychrome_sprites.offset,
                SpriteInputIndex::Sprites as usize,
            );
            command_encoder.setFragmentTexture_atIndex(
                Some(&texture),
                SpriteInputIndex::AtlasTexture as usize,
            );

            command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                MTLPrimitiveType::Triangle,
                0,
                6,
                sprites.len(),
                sprites.start,
            );
        }
    }

    fn draw_surfaces(
        &mut self,
        surfaces: &[PaintSurface],
        first_surface: usize,
        instance_bindings: &InstanceBindings,
        viewport_size: Size<DevicePixels>,
        command_encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) {
        if surfaces.is_empty() {
            return;
        }

        command_encoder.setRenderPipelineState(&self.surfaces_pipeline_state);
        // Safety: the surface range lies inside a buffer written for this
        // frame and the indices match the shader argument table.
        unsafe {
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&self.unit_vertices),
                0,
                SurfaceInputIndex::Vertices as usize,
            );
            command_encoder.setVertexBuffer_offset_atIndex(
                Some(&instance_bindings.surfaces.buffer),
                instance_bindings.surfaces.offset,
                SurfaceInputIndex::Surfaces as usize,
            );
            command_encoder.setVertexBytes_length_atIndex(
                NonNull::from(&viewport_size).cast(),
                mem::size_of_val(&viewport_size),
                SurfaceInputIndex::ViewportSize as usize,
            );
        }

        for (index, surface) in surfaces.iter().enumerate() {
            let image_buffer: &CVPixelBuffer = &surface.image_buffer;
            let texture_size = size(
                DevicePixels::from(CVPixelBufferGetWidth(image_buffer) as i32),
                DevicePixels::from(CVPixelBufferGetHeight(image_buffer) as i32),
            );

            assert_eq!(
                CVPixelBufferGetPixelFormatType(image_buffer),
                kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
            );

            let y_texture = create_texture_from_image(
                &self.core_video_texture_cache,
                image_buffer,
                MTLPixelFormat::R8Unorm,
                CVPixelBufferGetWidthOfPlane(image_buffer, 0),
                CVPixelBufferGetHeightOfPlane(image_buffer, 0),
                0,
            )
            .unwrap();
            let cb_cr_texture = create_texture_from_image(
                &self.core_video_texture_cache,
                image_buffer,
                MTLPixelFormat::RG8Unorm,
                CVPixelBufferGetWidthOfPlane(image_buffer, 1),
                CVPixelBufferGetHeightOfPlane(image_buffer, 1),
                1,
            )
            .unwrap();

            // Safety: the texture size is plain data matching the shader
            // argument, the textures come from the plane-sized texture cache
            // entries created above, and the base instance indexes the surface
            // bounds written for this frame.
            unsafe {
                command_encoder.setVertexBytes_length_atIndex(
                    NonNull::from(&texture_size).cast(),
                    mem::size_of_val(&texture_size),
                    SurfaceInputIndex::TextureSize as usize,
                );
                command_encoder.setFragmentTexture_atIndex(
                    CVMetalTextureGetTexture(&y_texture).as_deref(),
                    SurfaceInputIndex::YTexture as usize,
                );
                command_encoder.setFragmentTexture_atIndex(
                    CVMetalTextureGetTexture(&cb_cr_texture).as_deref(),
                    SurfaceInputIndex::CbCrTexture as usize,
                );

                command_encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                    MTLPrimitiveType::Triangle,
                    0,
                    6,
                    1,
                    first_surface + index,
                );
            }
        }
    }
}

fn new_command_encoder_for_texture(
    command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
    texture: &ProtocolObject<dyn MTLTexture>,
    viewport_size: Size<DevicePixels>,
    clear_color: Option<MTLClearColor>,
) -> Retained<ProtocolObject<dyn MTLRenderCommandEncoder>> {
    let render_pass_descriptor = MTLRenderPassDescriptor::new();
    // Safety: a render pass descriptor always provides color attachment 0.
    let color_attachment = unsafe {
        render_pass_descriptor
            .colorAttachments()
            .objectAtIndexedSubscript(0)
    };
    color_attachment.setTexture(Some(texture));
    color_attachment.setStoreAction(MTLStoreAction::Store);
    if let Some(clear_color) = clear_color {
        color_attachment.setLoadAction(MTLLoadAction::Clear);
        color_attachment.setClearColor(clear_color);
    } else {
        color_attachment.setLoadAction(MTLLoadAction::Load);
    }

    let command_encoder = command_buffer
        .renderCommandEncoderWithDescriptor(&render_pass_descriptor)
        .expect("failed to create render command encoder");
    command_encoder.setViewport(MTLViewport {
        originX: 0.0,
        originY: 0.0,
        width: i32::from(viewport_size.width) as f64,
        height: i32::from(viewport_size.height) as f64,
        znear: 0.0,
        zfar: 1.0,
    });
    command_encoder
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
fn read_texture_to_image(texture: &ProtocolObject<dyn MTLTexture>) -> Result<RgbaImage> {
    let width = texture.width() as u32;
    let height = texture.height() as u32;
    let bytes_per_row = width as usize * 4;
    let mut pixels = vec![0u8; height as usize * bytes_per_row];

    let region = MTLRegion {
        origin: MTLOrigin { x: 0, y: 0, z: 0 },
        size: MTLSize {
            width: width as usize,
            height: height as usize,
            depth: 1,
        },
    };
    // Safety: `pixels` holds exactly `height` rows of `bytes_per_row` bytes,
    // which covers the whole requested region of the BGRA8 texture.
    unsafe {
        texture.getBytes_bytesPerRow_fromRegion_mipmapLevel(
            NonNull::new(pixels.as_mut_ptr())
                .expect("vector pointers are never null")
                .cast::<c_void>(),
            bytes_per_row,
            region,
            0,
        );
    }

    // Convert BGRA to RGBA (swap B and R channels)
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }

    RgbaImage::from_raw(width, height, pixels).context("failed to create RgbaImage from pixel data")
}

fn new_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    descriptor: &MTLTextureDescriptor,
) -> Retained<ProtocolObject<dyn MTLTexture>> {
    device
        .newTextureWithDescriptor(descriptor)
        .expect("failed to create metal texture")
}

fn new_core_video_texture_cache(
    device: &ProtocolObject<dyn MTLDevice>,
) -> CFRetained<CVMetalTextureCache> {
    let mut cache = ptr::null_mut();
    // Safety: the out pointer is valid for writes and the device is a live
    // Metal device; no cache or texture attributes are passed.
    let result =
        unsafe { CVMetalTextureCache::create(None, None, device, None, NonNull::from(&mut cache)) };
    assert_eq!(
        result, kCVReturnSuccess,
        "could not create texture cache, code: {result}"
    );
    // Safety: on success the create function stores a +1 reference that the
    // caller owns.
    unsafe { CFRetained::from_raw(NonNull::new(cache).expect("texture cache was null")) }
}

fn create_texture_from_image(
    texture_cache: &CVMetalTextureCache,
    image_buffer: &CVPixelBuffer,
    pixel_format: MTLPixelFormat,
    width: usize,
    height: usize,
    plane_index: usize,
) -> Result<CFRetained<CVMetalTexture>> {
    let mut texture = ptr::null_mut();
    // Safety: the out pointer is valid for writes, and the plane index and
    // plane dimensions come from the same pixel buffer.
    let result = unsafe {
        CVMetalTextureCache::create_texture_from_image(
            None,
            texture_cache,
            image_buffer,
            None,
            pixel_format,
            width,
            height,
            plane_index,
            NonNull::from(&mut texture),
        )
    };
    anyhow::ensure!(
        result == kCVReturnSuccess,
        "could not create texture, code: {result}"
    );
    let texture = NonNull::new(texture).context("texture cache returned a null texture")?;
    // Safety: on success the create function stores a +1 reference that the
    // caller owns.
    Ok(unsafe { CFRetained::from_raw(texture) })
}

fn build_pipeline_state(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: MTLPixelFormat,
) -> Retained<ProtocolObject<dyn MTLRenderPipelineState>> {
    let vertex_fn = library
        .newFunctionWithName(&NSString::from_str(vertex_fn_name))
        .expect("error locating vertex function");
    let fragment_fn = library
        .newFunctionWithName(&NSString::from_str(fragment_fn_name))
        .expect("error locating fragment function");

    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setLabel(Some(&NSString::from_str(label)));
    descriptor.setVertexFunction(Some(&vertex_fn));
    descriptor.setFragmentFunction(Some(&fragment_fn));
    // Safety: a render pipeline descriptor always provides color attachment 0.
    let color_attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    color_attachment.setPixelFormat(pixel_format);
    color_attachment.setBlendingEnabled(true);
    color_attachment.setRgbBlendOperation(MTLBlendOperation::Add);
    color_attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
    color_attachment.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
    color_attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
    color_attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::One);

    device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .expect("could not create render pipeline state")
}

/// The reduction and blur passes overwrite every pixel of their target, so
/// they draw without blending.
fn build_backdrop_pass_pipeline_state(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
) -> Retained<ProtocolObject<dyn MTLRenderPipelineState>> {
    let vertex_fn = library
        .newFunctionWithName(&NSString::from_str(vertex_fn_name))
        .expect("error locating vertex function");
    let fragment_fn = library
        .newFunctionWithName(&NSString::from_str(fragment_fn_name))
        .expect("error locating fragment function");

    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setLabel(Some(&NSString::from_str(label)));
    descriptor.setVertexFunction(Some(&vertex_fn));
    descriptor.setFragmentFunction(Some(&fragment_fn));
    // Safety: a render pipeline descriptor always provides color attachment 0.
    let color_attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    color_attachment.setPixelFormat(BACKDROP_BLUR_FORMAT);
    color_attachment.setBlendingEnabled(false);

    device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .expect("could not create render pipeline state")
}

/// The composite pass replaces color inside the blurred shape and leaves the
/// destination alpha untouched. Its pixels come from that same destination, so
/// how much of the desktop a transparent window covers has not changed, and
/// recomputing it from the coverage this pass writes would push the region
/// toward opaque. The source color is scaled by the destination's alpha, which
/// puts it back into the frame's premultiplied form.
fn build_backdrop_composite_pipeline_state(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: MTLPixelFormat,
) -> Retained<ProtocolObject<dyn MTLRenderPipelineState>> {
    let vertex_fn = library
        .newFunctionWithName(&NSString::from_str(vertex_fn_name))
        .expect("error locating vertex function");
    let fragment_fn = library
        .newFunctionWithName(&NSString::from_str(fragment_fn_name))
        .expect("error locating fragment function");

    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setLabel(Some(&NSString::from_str(label)));
    descriptor.setVertexFunction(Some(&vertex_fn));
    descriptor.setFragmentFunction(Some(&fragment_fn));
    // Safety: a render pipeline descriptor always provides color attachment 0.
    let color_attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    color_attachment.setPixelFormat(pixel_format);
    color_attachment.setBlendingEnabled(true);
    color_attachment.setRgbBlendOperation(MTLBlendOperation::Add);
    color_attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
    color_attachment.setSourceRGBBlendFactor(MTLBlendFactor::DestinationAlpha);
    color_attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.setSourceAlphaBlendFactor(MTLBlendFactor::Zero);
    color_attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::One);

    device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .expect("could not create render pipeline state")
}

fn build_path_sprite_pipeline_state(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: MTLPixelFormat,
) -> Retained<ProtocolObject<dyn MTLRenderPipelineState>> {
    let vertex_fn = library
        .newFunctionWithName(&NSString::from_str(vertex_fn_name))
        .expect("error locating vertex function");
    let fragment_fn = library
        .newFunctionWithName(&NSString::from_str(fragment_fn_name))
        .expect("error locating fragment function");

    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setLabel(Some(&NSString::from_str(label)));
    descriptor.setVertexFunction(Some(&vertex_fn));
    descriptor.setFragmentFunction(Some(&fragment_fn));
    // Safety: a render pipeline descriptor always provides color attachment 0.
    let color_attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    color_attachment.setPixelFormat(pixel_format);
    color_attachment.setBlendingEnabled(true);
    color_attachment.setRgbBlendOperation(MTLBlendOperation::Add);
    color_attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
    color_attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
    color_attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
    color_attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::One);

    device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .expect("could not create render pipeline state")
}

fn build_path_rasterization_pipeline_state(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    pixel_format: MTLPixelFormat,
    path_sample_count: u32,
) -> Retained<ProtocolObject<dyn MTLRenderPipelineState>> {
    let vertex_fn = library
        .newFunctionWithName(&NSString::from_str(vertex_fn_name))
        .expect("error locating vertex function");
    let fragment_fn = library
        .newFunctionWithName(&NSString::from_str(fragment_fn_name))
        .expect("error locating fragment function");

    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setLabel(Some(&NSString::from_str(label)));
    descriptor.setVertexFunction(Some(&vertex_fn));
    descriptor.setFragmentFunction(Some(&fragment_fn));
    if path_sample_count > 1 {
        descriptor.setRasterSampleCount(path_sample_count as usize);
        descriptor.setAlphaToCoverageEnabled(false);
    }
    // Safety: a render pipeline descriptor always provides color attachment 0.
    let color_attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    color_attachment.setPixelFormat(pixel_format);
    color_attachment.setBlendingEnabled(true);
    color_attachment.setRgbBlendOperation(MTLBlendOperation::Add);
    color_attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
    color_attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
    color_attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
    color_attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    color_attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);

    device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .expect("could not create render pipeline state")
}

#[derive(Clone)]
struct InstanceBinding {
    buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    offset: usize,
}

struct InstanceBindings {
    quads: InstanceBinding,
    shadows: InstanceBinding,
    underlines: InstanceBinding,
    monochrome_sprites: InstanceBinding,
    polychrome_sprites: InstanceBinding,
    surfaces: InstanceBinding,
}

fn write_instances(scene: &Scene, writer: &mut InstanceBufferWriter) -> Result<InstanceBindings> {
    Ok(InstanceBindings {
        quads: writer.write(&scene.quads)?,
        shadows: writer.write(&scene.shadows)?,
        underlines: writer.write(&scene.underlines)?,
        monochrome_sprites: writer.write(&scene.monochrome_sprites)?,
        polychrome_sprites: writer.write(&scene.polychrome_sprites)?,
        surfaces: writer.write_iter(scene.surfaces.iter().map(|surface| SurfaceBounds {
            bounds: surface.bounds,
            content_mask: surface.content_mask,
        }))?,
    })
}

struct InstanceBufferWriter {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    pool: Arc<Mutex<InstanceBufferPool>>,
    unified_memory: bool,
    filled: Vec<(InstanceBuffer, usize)>,
    current: InstanceBuffer,
    offset: usize,
}

impl InstanceBufferWriter {
    fn new(
        device: &Retained<ProtocolObject<dyn MTLDevice>>,
        pool: &Arc<Mutex<InstanceBufferPool>>,
        unified_memory: bool,
    ) -> Self {
        let current = pool.lock().acquire(device, unified_memory);
        Self {
            device: device.clone(),
            pool: pool.clone(),
            unified_memory,
            filled: Vec::new(),
            current,
            offset: 0,
        }
    }

    fn allocate<T>(&mut self, count: usize) -> Result<(InstanceBinding, &mut [MaybeUninit<T>])> {
        let size = mem::size_of::<T>() * count;
        let mut offset = self.offset.next_multiple_of(INSTANCE_BUFFER_ALIGNMENT);
        if offset + size > self.current.size {
            self.grow(size)?;
            offset = 0;
        }
        self.offset = offset + size;

        let binding = InstanceBinding {
            buffer: self.current.metal_buffer.clone(),
            offset,
        };
        // Safety: the reservation lies within a buffer this frame owns
        // exclusively, and never overlaps one handed out earlier.
        let values = unsafe {
            let start = self
                .current
                .metal_buffer
                .contents()
                .cast::<u8>()
                .as_ptr()
                .add(offset);
            slice::from_raw_parts_mut(start.cast::<MaybeUninit<T>>(), count)
        };
        Ok((binding, values))
    }

    fn write<T>(&mut self, values: &[T]) -> Result<InstanceBinding> {
        let (binding, destination) = self.allocate::<T>(values.len())?;
        unsafe {
            ptr::copy_nonoverlapping(
                values.as_ptr(),
                destination.as_mut_ptr().cast::<T>(),
                values.len(),
            );
        }
        Ok(binding)
    }

    fn write_iter<T>(
        &mut self,
        values: impl ExactSizeIterator<Item = T>,
    ) -> Result<InstanceBinding> {
        let (binding, destination) = self.allocate::<T>(values.len())?;
        for (slot, value) in destination.iter_mut().zip(values) {
            slot.write(value);
        }
        Ok(binding)
    }

    fn grow(&mut self, required: usize) -> Result<()> {
        let mut pool = self.pool.lock();
        let buffer_size = (pool.buffer_size * 2)
            .max(required.next_power_of_two())
            .min(MAX_INSTANCE_BUFFER_SIZE);
        anyhow::ensure!(
            buffer_size >= required,
            "instance buffer needs {required} bytes, above the maximum of {MAX_INSTANCE_BUFFER_SIZE}"
        );
        anyhow::ensure!(
            buffer_size > self.current.size,
            "frame instance data exceeds the {MAX_INSTANCE_BUFFER_SIZE}-byte maximum"
        );
        if buffer_size != pool.buffer_size {
            log::info!("increased instance buffer size to {buffer_size}");
            pool.reset(buffer_size);
        }
        let buffer = pool.acquire(&self.device, self.unified_memory);
        drop(pool);

        let filled = mem::replace(&mut self.current, buffer);
        self.filled.push((filled, self.offset));
        self.offset = 0;
        Ok(())
    }

    fn finish(self) -> InstanceBuffer {
        let Self {
            unified_memory,
            filled,
            current,
            offset,
            ..
        } = self;

        if !unified_memory {
            for (buffer, written) in &filled {
                if *written == 0 {
                    continue;
                }
                buffer.metal_buffer.didModifyRange(NSRange {
                    location: 0,
                    length: *written,
                });
            }
            if offset > 0 {
                current.metal_buffer.didModifyRange(NSRange {
                    location: 0,
                    length: offset,
                });
            }
        }

        // Metal retains encoded resources until the command buffer completes.
        // Only the final, largest buffer is worth keeping in the pool.
        drop(filled);
        current
    }
}

#[repr(C)]
enum ShadowInputIndex {
    Vertices = 0,
    Shadows = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum QuadInputIndex {
    Vertices = 0,
    Quads = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum UnderlineInputIndex {
    Vertices = 0,
    Underlines = 1,
    ViewportSize = 2,
}

#[repr(C)]
enum SpriteInputIndex {
    Vertices = 0,
    Sprites = 1,
    ViewportSize = 2,
    AtlasTextureSize = 3,
    AtlasTexture = 4,
}

#[repr(C)]
enum SurfaceInputIndex {
    Vertices = 0,
    Surfaces = 1,
    ViewportSize = 2,
    TextureSize = 3,
    YTexture = 4,
    CbCrTexture = 5,
}

#[repr(C)]
enum PathRasterizationInputIndex {
    Vertices = 0,
    ViewportSize = 1,
}

#[repr(C)]
enum BackdropBlurInputIndex {
    Vertices = 0,
    Params = 1,
    ViewportSize = 2,
    Source = 3,
}

/// One reduction or blur pass over the reduced backdrop.
#[derive(Clone, Copy)]
#[repr(C)]
struct BackdropBlurPass {
    bounds: Bounds<ScaledPixels>,
    target_size: [f32; 2],
    source_size: [f32; 2],
    /// Unit vector along which this pass accumulates; the separable kernel
    /// needs one pass per axis.
    direction: [f32; 2],
    sigma: f32,
    /// Source pixels covered by one target pixel, so the reduction can place
    /// its taps; the blur passes run at 1:1 and leave it at one.
    source_scale: f32,
    /// Sampling window for this pass, in source pixel centers.
    source_min: [f32; 2],
    source_max: [f32; 2],
}

/// Draws the blurred backdrop back into one region's rounded shape.
#[derive(Clone, Copy)]
#[repr(C)]
struct BackdropBlurSprite {
    bounds: Bounds<ScaledPixels>,
    content_mask: ContentMask<ScaledPixels>,
    corner_radii: Corners<ScaledPixels>,
    source_size: [f32; 2],
    /// Frame pixels covered by one blurred-texture pixel.
    source_scale: f32,
    opacity: f32,
}

/// Offscreen textures for backdrop blurs, sized to the frame.
struct BackdropBlurTargets {
    size: Size<DevicePixels>,
    /// Snapshot of the frame so far. The drawable cannot be sampled while it
    /// is also the render target, so the reduction reads this copy.
    source: Retained<ProtocolObject<dyn MTLTexture>>,
    /// Ping-pong pair at the reduced resolution.
    scratch: [Retained<ProtocolObject<dyn MTLTexture>>; 2],
    /// Reduced resolution in pixels, as the shaders want it.
    reduced_size: [f32; 2],
}

impl BackdropBlurTargets {
    fn new(device: &ProtocolObject<dyn MTLDevice>, size: Size<DevicePixels>) -> Self {
        let width = size.width.0 as usize;
        let height = size.height.0 as usize;

        let source_descriptor = MTLTextureDescriptor::new();
        // Safety: the caller skips blurs on frames with a zero dimension.
        unsafe {
            source_descriptor.setWidth(width);
            source_descriptor.setHeight(height);
        }
        source_descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        source_descriptor.setStorageMode(MTLStorageMode::Private);
        source_descriptor.setUsage(MTLTextureUsage::ShaderRead);
        let source = new_texture(device, &source_descriptor);

        let reduced_width = width.div_ceil(BACKDROP_BLUR_DOWNSCALE).max(1);
        let reduced_height = height.div_ceil(BACKDROP_BLUR_DOWNSCALE).max(1);
        let scratch_descriptor = MTLTextureDescriptor::new();
        // Safety: both reduced dimensions are at least one.
        unsafe {
            scratch_descriptor.setWidth(reduced_width);
            scratch_descriptor.setHeight(reduced_height);
        }
        scratch_descriptor.setPixelFormat(BACKDROP_BLUR_FORMAT);
        scratch_descriptor.setStorageMode(MTLStorageMode::Private);
        scratch_descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        let scratch = [
            new_texture(device, &scratch_descriptor),
            new_texture(device, &scratch_descriptor),
        ];

        Self {
            size,
            source,
            scratch,
            reduced_size: [reduced_width as f32, reduced_height as f32],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct PathSprite {
    pub bounds: Bounds<ScaledPixels>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct SurfaceBounds {
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
pub struct MetalHeadlessRenderer {
    renderer: MetalRenderer,
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
impl MetalHeadlessRenderer {
    pub fn new() -> Self {
        let instance_buffer_pool = Arc::new(Mutex::new(InstanceBufferPool::default()));
        let renderer = MetalRenderer::new_headless(instance_buffer_pool);
        Self { renderer }
    }
}

#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
impl gpui::PlatformHeadlessRenderer for MetalHeadlessRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> anyhow::Result<image::RgbaImage> {
        self.renderer.render_scene_to_image(scene, size)
    }

    fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> anyhow::Result<()> {
        self.renderer.render_scene(scene, size)
    }

    fn sprite_atlas(&self) -> Arc<dyn gpui::PlatformAtlas> {
        self.renderer.sprite_atlas().clone()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        BackdropBlur, Bounds, ContentMask, DevicePixels, PlatformHeadlessRenderer, Quad,
        ScaledPixels, Scene, black, point, red, size, white,
    };

    use crate::metal_renderer::MetalHeadlessRenderer;

    #[test]
    fn backdrop_blur_softens_only_its_region_and_keeps_later_primitives() {
        let bounds = |x, y, width, height| Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(y)),
            size: size(ScaledPixels(width), ScaledPixels(height)),
        };
        let content_mask = ContentMask {
            bounds: bounds(0., 0., 64., 64.),
        };

        let mut scene = Scene::default();
        for (x, color) in [(0., black()), (32., white())] {
            scene.insert_primitive(Quad {
                bounds: bounds(x, 0., 32., 64.),
                content_mask,
                background: color.into(),
                ..Default::default()
            });
        }
        scene.insert_primitive(BackdropBlur {
            bounds: bounds(8., 8., 48., 48.),
            content_mask,
            sigma: ScaledPixels(4.),
            opacity: 1.,
            ..Default::default()
        });
        // Painted after the blur, so it must come out untouched.
        scene.insert_primitive(Quad {
            bounds: bounds(52., 54., 10., 8.),
            content_mask,
            background: red().into(),
            ..Default::default()
        });
        scene.finish();

        let mut renderer = MetalHeadlessRenderer::new();
        let image = renderer
            .render_scene_to_image(&scene, size(DevicePixels(64), DevicePixels(64)))
            .expect("the scene renders");
        let pixel = |x, y| image.get_pixel(x, y).0;

        // Outside the region the black half stays black.
        assert_eq!(pixel(28, 4), [0, 0, 0, 255]);

        // Inside it, next to the edge between the halves, black and white mix.
        let blurred = pixel(28, 32);
        assert!(
            blurred[0] > 10 && blurred[0] < 245,
            "blurred pixel {blurred:?}"
        );
        assert_eq!(blurred[3], 255);

        // Far from that edge the region keeps its own color.
        assert!(pixel(12, 32)[0] < 10, "left of region {:?}", pixel(12, 32));
        assert!(
            pixel(52, 32)[0] > 245,
            "right of region {:?}",
            pixel(52, 32)
        );

        assert_eq!(pixel(58, 58), [255, 0, 0, 255]);
    }
}
