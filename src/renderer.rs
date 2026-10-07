//! xrcomp's own renderer.
//!
//! This started as a copy of Smithay's `GlowRenderer` (`src/backend/renderer/glow.rs`
//! at rev 79bbed5e, MIT licensed): a thin wrapper around [`GlesRenderer`] that also
//! holds a [`glow::Context`] for hand-written OpenGL ES. Every trait method just
//! forwards to the wrapped `GlesRenderer`/`GlesFrame`, so behavior is identical to
//! Smithay's renderer until one of those methods is changed here.

use std::{
    borrow::{Borrow, BorrowMut},
    sync::Arc,
};

use glow::Context;
use smithay::{
    backend::{
        allocator::{Format, Fourcc, dmabuf::Dmabuf, format::FormatSet},
        egl::{EGLContext, display::EGLBufferReader},
        renderer::{
            Bind, Blit, BlitFrame, Color32F, ContextId, DebugFlags, ExportMem, Frame, FrameContext,
            ImportDma, ImportDmaWl, ImportEgl, ImportMem, ImportMemWl, Offscreen, Renderer,
            RendererSuper, TextureFilter,
            element::{RenderElement, UnderlyingStorage},
            gles::{
                Capability, GlesError, GlesFrame, GlesFrameGuard, GlesMapping, GlesRenderer, GlesTarget,
                GlesTexture,
                element::{PixelShaderElement, TextureShaderElement},
            },
            sync::SyncPoint,
        },
    },
    reexports::wayland_server::{
        DisplayHandle,
        protocol::{wl_buffer, wl_shm},
    },
    utils::{Buffer as BufferCoord, Physical, Point, Rectangle, Scale, Size, Transform, user_data::UserDataMap},
    wayland::compositor::SurfaceData,
};
use tracing::warn;

#[derive(Debug)]
pub struct XrRenderer {
    gl: GlesInternal,
    glow: Arc<Context>,
}

/// Either a `GlesRenderer` we own, or (inside [`FrameContext::renderer`]) a guard
/// borrowing the one owned by an in-progress frame.
#[derive(Debug)]
enum GlesInternal {
    Owned(Box<GlesRenderer>),
    Frame(GlesFrameGuard<'static, 'static, 'static>),
}

impl AsRef<GlesRenderer> for GlesInternal {
    fn as_ref(&self) -> &GlesRenderer {
        match self {
            Self::Owned(r) => r,
            Self::Frame(g) => g.as_ref(),
        }
    }
}

impl AsMut<GlesRenderer> for GlesInternal {
    fn as_mut(&mut self) -> &mut GlesRenderer {
        match self {
            Self::Owned(r) => r,
            Self::Frame(g) => g.as_mut(),
        }
    }
}

/// [`Frame`] implementation of [`XrRenderer`].
#[derive(Debug)]
pub struct XrFrame<'frame, 'buffer> {
    frame: Option<GlesFrame<'frame, 'buffer>>,
    glow: Arc<Context>,
}

impl XrRenderer {
    /// # Safety
    ///
    /// The given `EGLContext` must not be active in another thread.
    pub unsafe fn new(context: EGLContext) -> Result<Self, GlesError> {
        let capabilities = unsafe { GlesRenderer::supported_capabilities(&context)? };
        unsafe { Self::with_capabilities(context, capabilities) }
    }

    /// # Safety
    ///
    /// The given `EGLContext` must not be active in another thread.
    pub unsafe fn with_capabilities(
        context: EGLContext,
        capabilities: impl IntoIterator<Item = Capability>,
    ) -> Result<Self, GlesError> {
        let glow = unsafe {
            context.make_current()?;
            Context::from_loader_function(|s| smithay::backend::egl::get_proc_address(s) as *const _)
        };
        let gl = unsafe { GlesRenderer::with_capabilities(context, capabilities)? };

        Ok(Self {
            gl: GlesInternal::Owned(Box::new(gl)),
            glow: Arc::new(glow),
        })
    }

    pub fn egl_context(&self) -> &EGLContext {
        self.gl.as_ref().egl_context()
    }

    /// Run custom GL code outside of a frame. Any GL state changed must be restored,
    /// as `GlesRenderer` assumes it owns the context state.
    pub fn with_context<F, R>(&mut self, func: F) -> Result<R, GlesError>
    where
        F: FnOnce(&Arc<Context>) -> R,
    {
        unsafe {
            self.gl.as_ref().egl_context().make_current()?;
        }
        Ok(func(&self.glow))
    }
}

impl XrFrame<'_, '_> {
    /// Run custom GL code inside a frame (the target framebuffer is bound). Any GL
    /// state changed must be restored, as `GlesFrame` assumes it owns the context state.
    pub fn with_context<F, R>(&mut self, func: F) -> Result<R, GlesError>
    where
        F: FnOnce(&Arc<Context>) -> R,
    {
        Ok(func(&self.glow))
    }
}

// Needed by `smithay::backend::winit::init`, which always creates a `GlesRenderer`.
impl From<GlesRenderer> for XrRenderer {
    fn from(renderer: GlesRenderer) -> Self {
        let glow = unsafe {
            renderer.egl_context().make_current().unwrap();
            Context::from_loader_function(|s| smithay::backend::egl::get_proc_address(s) as *const _)
        };

        Self {
            gl: GlesInternal::Owned(Box::new(renderer)),
            glow: Arc::new(glow),
        }
    }
}

impl Borrow<GlesRenderer> for XrRenderer {
    fn borrow(&self) -> &GlesRenderer {
        self.gl.as_ref()
    }
}

impl BorrowMut<GlesRenderer> for XrRenderer {
    fn borrow_mut(&mut self) -> &mut GlesRenderer {
        self.gl.as_mut()
    }
}

impl<'frame, 'buffer> Borrow<GlesFrame<'frame, 'buffer>> for XrFrame<'frame, 'buffer> {
    fn borrow(&self) -> &GlesFrame<'frame, 'buffer> {
        self.frame.as_ref().unwrap()
    }
}

impl<'frame, 'buffer> BorrowMut<GlesFrame<'frame, 'buffer>> for XrFrame<'frame, 'buffer> {
    fn borrow_mut(&mut self) -> &mut GlesFrame<'frame, 'buffer> {
        self.frame.as_mut().unwrap()
    }
}

impl RendererSuper for XrRenderer {
    type Error = GlesError;
    type TextureId = GlesTexture;
    type Framebuffer<'buffer> = GlesTarget<'buffer>;
    type Frame<'frame, 'buffer>
        = XrFrame<'frame, 'buffer>
    where
        'buffer: 'frame,
        Self: 'frame;
}

impl Renderer for XrRenderer {
    fn context_id(&self) -> ContextId<GlesTexture> {
        self.gl.as_ref().context_id()
    }

    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.gl.as_mut().downscale_filter(filter)
    }
    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.gl.as_mut().upscale_filter(filter)
    }

    fn set_debug_flags(&mut self, flags: DebugFlags) {
        self.gl.as_mut().set_debug_flags(flags)
    }
    fn debug_flags(&self) -> DebugFlags {
        self.gl.as_ref().debug_flags()
    }

    fn render<'frame, 'buffer>(
        &'frame mut self,
        target: &'frame mut GlesTarget<'buffer>,
        output_size: Size<i32, Physical>,
        transform: Transform,
    ) -> Result<XrFrame<'frame, 'buffer>, Self::Error>
    where
        'buffer: 'frame,
    {
        let glow = self.glow.clone();
        let frame = self.gl.as_mut().render(target, output_size, transform)?;
        Ok(XrFrame {
            frame: Some(frame),
            glow,
        })
    }

    fn wait(&mut self, sync: &SyncPoint) -> Result<(), Self::Error> {
        self.gl.as_mut().wait(sync)
    }

    fn cleanup_texture_cache(&mut self) -> Result<(), Self::Error> {
        self.gl.as_mut().cleanup_texture_cache()
    }

    fn invalidate_caches(&mut self) -> Result<(), Self::Error> {
        self.gl.as_mut().invalidate_caches()
    }
}

impl Frame for XrFrame<'_, '_> {
    type Error = GlesError;
    type TextureId = GlesTexture;

    fn context_id(&self) -> ContextId<GlesTexture> {
        self.frame.as_ref().unwrap().context_id()
    }

    fn clear(&mut self, color: Color32F, at: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
        self.frame.as_mut().unwrap().clear(color, at)
    }

    fn draw_solid(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: Color32F,
    ) -> Result<(), Self::Error> {
        self.frame.as_mut().unwrap().draw_solid(dst, damage, color)
    }

    fn render_texture_from_to(
        &mut self,
        texture: &Self::TextureId,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        src_transform: Transform,
        alpha: f32,
    ) -> Result<(), Self::Error> {
        Frame::render_texture_from_to(
            self.frame.as_mut().unwrap(),
            texture,
            src,
            dst,
            damage,
            opaque_regions,
            src_transform,
            alpha,
        )
    }

    fn transformation(&self) -> Transform {
        self.frame.as_ref().unwrap().transformation()
    }
    fn output_size(&self) -> Size<i32, Physical> {
        self.frame.as_ref().unwrap().output_size()
    }

    fn render_texture_at(
        &mut self,
        texture: &Self::TextureId,
        pos: Point<i32, Physical>,
        texture_scale: i32,
        output_scale: impl Into<Scale<f64>>,
        src_transform: Transform,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        alpha: f32,
    ) -> Result<(), Self::Error> {
        self.frame.as_mut().unwrap().render_texture_at(
            texture,
            pos,
            texture_scale,
            output_scale,
            src_transform,
            damage,
            opaque_regions,
            alpha,
        )
    }

    fn wait(&mut self, sync: &SyncPoint) -> Result<(), Self::Error> {
        self.frame.as_mut().unwrap().wait(sync)
    }

    fn finish(mut self) -> Result<SyncPoint, Self::Error> {
        self.finish_internal()
    }
}

impl XrFrame<'_, '_> {
    fn finish_internal(&mut self) -> Result<SyncPoint, GlesError> {
        if let Some(frame) = self.frame.take() {
            frame.finish()
        } else {
            Ok(SyncPoint::default())
        }
    }
}

impl Drop for XrFrame<'_, '_> {
    fn drop(&mut self) {
        if let Err(err) = self.finish_internal() {
            warn!("Ignored error finishing XrFrame on drop: {}", err);
        }
    }
}

/// Guard type wrapping the underlying [`XrRenderer`] of an [`XrFrame`].
#[derive(Debug)]
pub struct XrFrameGuard<'a, 'frame, 'buffer> {
    renderer: XrRenderer,
    _phantom: std::marker::PhantomData<(&'a (), &'frame (), &'buffer ())>,
}

impl AsRef<XrRenderer> for XrFrameGuard<'_, '_, '_> {
    fn as_ref(&self) -> &XrRenderer {
        &self.renderer
    }
}

impl AsMut<XrRenderer> for XrFrameGuard<'_, '_, '_> {
    fn as_mut(&mut self) -> &mut XrRenderer {
        &mut self.renderer
    }
}

impl<'a, 'frame, 'buffer> FrameContext<'a, 'frame, 'buffer, XrRenderer> for XrFrame<'frame, 'buffer>
where
    'frame: 'a,
{
    type Guard = XrFrameGuard<'a, 'frame, 'buffer>;

    fn renderer(&'a mut self) -> Self::Guard {
        let guard = self.frame.as_mut().unwrap().renderer();

        XrFrameGuard {
            renderer: XrRenderer {
                // Same lifetime erasure as Smithay's `GlowRenderer`; `_phantom` keeps the
                // real lifetimes attached to the guard.
                gl: GlesInternal::Frame(unsafe {
                    std::mem::transmute::<
                        GlesFrameGuard<'a, 'frame, 'buffer>,
                        GlesFrameGuard<'static, 'static, 'static>,
                    >(guard)
                }),
                glow: self.glow.clone(),
            },
            _phantom: std::marker::PhantomData,
        }
    }
}

impl ImportMemWl for XrRenderer {
    fn import_shm_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, BufferCoord>],
    ) -> Result<GlesTexture, GlesError> {
        self.gl.as_mut().import_shm_buffer(buffer, surface, damage)
    }

    fn shm_formats(&self) -> Box<dyn Iterator<Item = wl_shm::Format>> {
        self.gl.as_ref().shm_formats()
    }
}

impl ImportMem for XrRenderer {
    fn import_memory(
        &mut self,
        data: &[u8],
        format: Fourcc,
        size: Size<i32, BufferCoord>,
        flipped: bool,
    ) -> Result<GlesTexture, GlesError> {
        self.gl.as_mut().import_memory(data, format, size, flipped)
    }

    fn update_memory(
        &mut self,
        texture: &Self::TextureId,
        data: &[u8],
        region: Rectangle<i32, BufferCoord>,
    ) -> Result<(), Self::Error> {
        self.gl.as_mut().update_memory(texture, data, region)
    }

    fn mem_formats(&self) -> Box<dyn Iterator<Item = Fourcc>> {
        self.gl.as_ref().mem_formats()
    }
}

impl ImportEgl for XrRenderer {
    fn bind_wl_display(&mut self, display: &DisplayHandle) -> Result<(), smithay::backend::egl::Error> {
        self.gl.as_mut().bind_wl_display(display)
    }

    fn unbind_wl_display(&mut self) {
        self.gl.as_mut().unbind_wl_display()
    }

    fn egl_reader(&self) -> Option<&EGLBufferReader> {
        self.gl.as_ref().egl_reader()
    }

    fn import_egl_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, BufferCoord>],
    ) -> Result<GlesTexture, GlesError> {
        self.gl.as_mut().import_egl_buffer(buffer, surface, damage)
    }
}

impl ImportDma for XrRenderer {
    fn import_dmabuf(
        &mut self,
        buffer: &Dmabuf,
        damage: Option<&[Rectangle<i32, BufferCoord>]>,
    ) -> Result<GlesTexture, GlesError> {
        self.gl.as_mut().import_dmabuf(buffer, damage)
    }
    fn dmabuf_formats(&self) -> FormatSet {
        self.gl.as_ref().dmabuf_formats()
    }
    fn has_dmabuf_format(&self, format: Format) -> bool {
        self.gl.as_ref().has_dmabuf_format(format)
    }
}

impl ImportDmaWl for XrRenderer {}

impl ExportMem for XrRenderer {
    type TextureMapping = GlesMapping;

    fn copy_framebuffer(
        &mut self,
        from: &GlesTarget<'_>,
        region: Rectangle<i32, BufferCoord>,
        format: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        self.gl.as_mut().copy_framebuffer(from, region, format)
    }

    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, BufferCoord>,
        format: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        self.gl.as_mut().copy_texture(texture, region, format)
    }

    fn can_read_texture(&mut self, texture: &Self::TextureId) -> Result<bool, Self::Error> {
        self.gl.as_mut().can_read_texture(texture)
    }

    fn map_texture<'a>(&mut self, texture_mapping: &'a Self::TextureMapping) -> Result<&'a [u8], Self::Error> {
        self.gl.as_mut().map_texture(texture_mapping)
    }
}

impl<T> Bind<T> for XrRenderer
where
    GlesRenderer: Bind<T>,
{
    fn bind<'a>(&mut self, target: &'a mut T) -> Result<GlesTarget<'a>, GlesError> {
        self.gl.as_mut().bind(target)
    }
    fn supported_formats(&self) -> Option<FormatSet> {
        self.gl.as_ref().supported_formats()
    }
}

impl<T> Offscreen<T> for XrRenderer
where
    GlesRenderer: Offscreen<T>,
{
    fn create_buffer(&mut self, format: Fourcc, size: Size<i32, BufferCoord>) -> Result<T, GlesError> {
        self.gl.as_mut().create_buffer(format, size)
    }
}

impl<'buffer> BlitFrame<GlesTarget<'buffer>> for XrFrame<'_, 'buffer> {
    fn blit_to(
        &mut self,
        to: &mut GlesTarget<'buffer>,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<SyncPoint, Self::Error> {
        self.frame.as_mut().unwrap().blit_to(to, src, dst, filter)
    }

    fn blit_from(
        &mut self,
        from: &GlesTarget<'buffer>,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<SyncPoint, Self::Error> {
        self.frame.as_mut().unwrap().blit_from(from, src, dst, filter)
    }
}

impl Blit for XrRenderer {
    fn blit(
        &mut self,
        from: &GlesTarget<'_>,
        to: &mut GlesTarget<'_>,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<SyncPoint, GlesError> {
        self.gl.as_mut().blit(from, to, src, dst, filter)
    }
}

// Let Smithay's custom-shader elements (`GlesRenderer::compile_custom_pixel_shader` etc.)
// be drawn with this renderer too, by delegating to their `GlesRenderer` impls.
impl RenderElement<XrRenderer> for PixelShaderElement {
    fn draw(
        &self,
        frame: &mut XrFrame<'_, '_>,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        RenderElement::<GlesRenderer>::draw(self, frame.borrow_mut(), src, dst, damage, opaque_regions, cache)
    }

    fn underlying_storage(&self, renderer: &mut XrRenderer) -> Option<UnderlyingStorage<'_>> {
        RenderElement::<GlesRenderer>::underlying_storage(self, renderer.borrow_mut())
    }
}

impl RenderElement<XrRenderer> for TextureShaderElement {
    fn draw(
        &self,
        frame: &mut XrFrame<'_, '_>,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        RenderElement::<GlesRenderer>::draw(self, frame.borrow_mut(), src, dst, damage, opaque_regions, cache)
    }

    fn underlying_storage(&self, renderer: &mut XrRenderer) -> Option<UnderlyingStorage<'_>> {
        RenderElement::<GlesRenderer>::underlying_storage(self, renderer.borrow_mut())
    }
}
