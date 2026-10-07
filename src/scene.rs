//! The 3D scene: draws every window as a textured quad positioned in 3D space.
//!
//! Rendering happens in two stages:
//!
//! 1. [`Scene::prepare`] renders each window (including its subsurfaces and popups) flat into
//!    its own offscreen texture, using Smithay's normal 2D pipeline and damage tracking.
//! 2. The [`SceneElement`] it returns is a single full-output render element whose `draw`
//!    uses hand-written GL ES (via glow) to draw those textures as quads with our own
//!    view/projection matrices. Drawing a window from several perspectives just means
//!    sampling its texture several times.
//!
//! The camera is set up so that a quad at z = 0 lands on exactly the same pixels the window
//! would occupy in plain 2D. As long as windows stay on that plane, the 2D `Space` layout
//! used for input (`surface_under` etc.) still lines up with what is drawn.

use std::{collections::HashMap, num::NonZeroU32};

use glam::{
    Affine2, Mat4, Vec3, Vec4,
    camera::rh::{proj::opengl::perspective, view::look_at_mat4},
};
use glow::HasContext;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, Frame, Offscreen, Texture,
            damage::OutputDamageTracker,
            element::{
                AsRenderElements, Element, Id, RenderElement, surface::WaylandSurfaceRenderElement,
            },
            gles::{GlesError, GlesTexture},
            glow::{GlowFrame, GlowRenderer},
            utils::CommitCounter,
        },
    },
    desktop::{Space, Window},
    output::Output,
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform, user_data::UserDataMap},
};
use tracing::warn;

const VERTEX_SHADER: &str = r#"#version 100
attribute vec2 a_pos;
uniform mat4 u_mvp;
varying vec2 v_uv;
void main() {
    v_uv = a_pos;
    gl_Position = u_mvp * vec4(a_pos, 0.0, 1.0);
}
"#;

// Window textures hold premultiplied alpha, matching the blend function Smithay sets up
// at the start of every frame (`ONE, ONE_MINUS_SRC_ALPHA`).
const FRAGMENT_SHADER: &str = r#"#version 100
precision mediump float;
uniform sampler2D u_tex;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv);
}
"#;

/// Vertical field of view of the scene camera.
const FOV_Y: f32 = std::f32::consts::FRAC_PI_3;

/// Per-backend scene state; lives next to the renderer in each backend's data.
#[derive(Debug)]
pub struct Scene {
    gl: Option<SceneGl>,
    windows: HashMap<Window, WindowTexture>,
    id: Id,
    commit: CommitCounter,
}

/// A window rendered flat into an offscreen texture (stage 1).
#[derive(Debug)]
struct WindowTexture {
    texture: GlesTexture,
    damage_tracker: OutputDamageTracker,
    /// Whether the texture still holds a previous frame, i.e. the buffer age to render with.
    age: usize,
}

impl Scene {
    pub fn new() -> Self {
        Self {
            gl: None,
            windows: HashMap::new(),
            id: Id::new(),
            commit: CommitCounter::default(),
        }
    }

    /// Re-render any damaged window textures and build the element that draws the scene.
    ///
    /// Must be called outside of a frame, since it binds and renders into the window textures.
    pub fn prepare(
        &mut self,
        renderer: &mut GlowRenderer,
        space: &Space<Window>,
        output: &Output,
    ) -> Option<SceneElement> {
        if self.gl.is_none() {
            match renderer.with_context(|gl| unsafe { SceneGl::new(gl) }) {
                Ok(Ok(gl)) => self.gl = Some(gl),
                Ok(Err(err)) => {
                    warn!("Failed to set up scene GL resources: {}", err);
                    return None;
                }
                Err(err) => {
                    warn!("Failed to make GL context current: {}", err);
                    return None;
                }
            }
        }

        let output_geo = space.output_geometry(output)?;
        let scale = Scale::from(output.current_scale().fractional_scale());

        self.windows.retain(|window, _| space.elements().any(|w| w == window));

        // `Space::elements` is back to front, which is the order to draw in while all
        // windows are on the same plane.
        let mut quads = Vec::new();
        for window in space.elements() {
            let bbox = window.bbox_with_popups();
            if bbox.is_empty() {
                continue;
            }
            let texture = match self.render_window_texture(renderer, window, bbox, scale) {
                Ok(texture) => texture,
                Err(err) => {
                    warn!("Failed to render window texture: {}", err);
                    continue;
                }
            };

            // World units are logical pixels, with x right, y down and z into the screen
            // (away from the viewer); a right-handed system.
            // The surface origin sits at the window's space location minus its geometry
            // offset (as in `Space`), and the texture covers `bbox` relative to that origin.
            let Some(location) = space.element_location(window) else {
                continue;
            };
            let origin = location - window.geometry().loc + bbox.loc;
            let model = window_placement(window)
                * Mat4::from_translation(Vec3::new(origin.x as f32, origin.y as f32, 0.0))
                * Mat4::from_scale(Vec3::new(bbox.size.w as f32, bbox.size.h as f32, 1.0));

            quads.push(Quad { texture, model });
        }

        self.commit.increment();
        Some(SceneElement {
            id: self.id.clone(),
            commit: self.commit,
            size: output_geo.size.to_physical_precise_round(scale),
            gl: self.gl?,
            view_proj: camera_view_proj(output_geo),
            quads,
        })
    }

    fn render_window_texture(
        &mut self,
        renderer: &mut GlowRenderer,
        window: &Window,
        bbox: Rectangle<i32, Logical>,
        scale: Scale<f64>,
    ) -> Result<GlesTexture, GlesError> {
        let size = bbox.size.to_physical_precise_round(scale);
        let buffer_size = (size.w, size.h).into();

        let needs_new_texture = self
            .windows
            .get(window)
            .is_none_or(|entry| entry.texture.size() != buffer_size);
        if needs_new_texture {
            let texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
            self.windows.insert(
                window.clone(),
                WindowTexture {
                    texture,
                    damage_tracker: OutputDamageTracker::new(size, scale, Transform::Normal),
                    age: 0,
                },
            );
        }
        let entry = self.windows.get_mut(window).unwrap();

        // Place the bbox's top-left corner at the texture's origin.
        let location = Point::<i32, Logical>::from((-bbox.loc.x, -bbox.loc.y)).to_physical_precise_round(scale);
        let elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
            window.render_elements(renderer, location, scale, 1.0);

        let mut target = renderer.bind(&mut entry.texture)?;
        entry
            .damage_tracker
            .render_output(renderer, &mut target, entry.age, &elements, Color32F::TRANSPARENT)
            .map_err(|err| match err {
                smithay::backend::renderer::damage::Error::Rendering(err) => err,
                smithay::backend::renderer::damage::Error::OutputNoMode(_) => GlesError::UnknownSize,
            })?;
        drop(target);
        entry.age = 1;

        Ok(entry.texture.clone())
    }
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

/// Where a window sits in 3D, applied on top of its 2D layout position.
///
/// This is the hook for 3D placement; the identity keeps every window on the z = 0 plane,
/// where it lines up exactly with its 2D position.
fn window_placement(_window: &Window) -> Mat4 {
    Mat4::IDENTITY
}

/// A perspective camera looking straight at the output's rectangle on the z = 0 plane,
/// from the distance at which that rectangle exactly fills the view.
fn camera_view_proj(output_geo: Rectangle<i32, Logical>) -> Mat4 {
    let w = output_geo.size.w as f32;
    let h = output_geo.size.h as f32;
    let center = Vec3::new(output_geo.loc.x as f32 + w / 2.0, output_geo.loc.y as f32 + h / 2.0, 0.0);
    let distance = (h / 2.0) / (FOV_Y / 2.0).tan();

    // World y points down, so "up" for the camera is -y.
    let view = look_at_mat4(center - Vec3::new(0.0, 0.0, distance), center, Vec3::NEG_Y);
    let proj = perspective(FOV_Y, w / h, distance / 100.0, distance * 100.0);
    proj * view
}

/// Converts standard GL clip space (+y up) into what Smithay's frame expects for the bound
/// target and output transform. This mirrors the `flip180 * transform.matrix()` part of
/// the projection in `GlesRenderer::render`.
fn target_matrix(transform: Transform) -> Mat4 {
    let flip180 = Affine2::from_cols_array(&[1.0, 0.0, 0.0, -1.0, 0.0, 0.0]);
    let m = flip180 * transform.matrix();
    Mat4::from_cols(
        Vec4::new(m.x_axis.x, m.x_axis.y, 0.0, 0.0),
        Vec4::new(m.y_axis.x, m.y_axis.y, 0.0, 0.0),
        Vec4::Z,
        Vec4::W,
    )
}

#[derive(Debug, Clone)]
struct Quad {
    texture: GlesTexture,
    /// Maps the unit square (0..1, 0..1) to the window's place in world space.
    model: Mat4,
}

/// GL objects for drawing the scene, created once per renderer.
#[derive(Debug, Clone, Copy)]
struct SceneGl {
    program: glow::Program,
    vbo: glow::Buffer,
    a_pos: u32,
    u_mvp: glow::UniformLocation,
    u_tex: glow::UniformLocation,
    /// GLES 3+ supports instancing, which Smithay uses and leaves divisors set for.
    reset_divisor: bool,
}

impl SceneGl {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, source) in [
                (glow::VERTEX_SHADER, VERTEX_SHADER),
                (glow::FRAGMENT_SHADER, FRAGMENT_SHADER),
            ] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, source);
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    return Err(gl.get_shader_info_log(shader));
                }
                gl.attach_shader(program, shader);
                shaders.push(shader);
            }
            gl.link_program(program);
            for shader in shaders {
                gl.detach_shader(program, shader);
                gl.delete_shader(shader);
            }
            if !gl.get_program_link_status(program) {
                return Err(gl.get_program_info_log(program));
            }

            let a_pos = gl
                .get_attrib_location(program, "a_pos")
                .ok_or("missing attribute a_pos")?;
            let u_mvp = gl
                .get_uniform_location(program, "u_mvp")
                .ok_or("missing uniform u_mvp")?;
            let u_tex = gl
                .get_uniform_location(program, "u_tex")
                .ok_or("missing uniform u_tex")?;

            // Unit square as a triangle strip; doubles as texture coordinates.
            let vertices: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
            let bytes: Vec<u8> = vertices.iter().flat_map(|v| v.to_ne_bytes()).collect();
            let vbo = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &bytes, glow::STATIC_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            Ok(Self {
                program,
                vbo,
                a_pos,
                u_mvp,
                u_tex,
                reset_divisor: gl.version().major >= 3,
            })
        }
    }

    /// Draws `quads` in order. Leaves the GL state the way Smithay expects it: blending and
    /// viewport untouched, and no program, buffer or texture bound.
    unsafe fn draw(&self, gl: &glow::Context, view_proj: Mat4, quads: &[Quad]) {
        unsafe {
            gl.use_program(Some(self.program));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.enable_vertex_attrib_array(self.a_pos);
            gl.vertex_attrib_pointer_f32(self.a_pos, 2, glow::FLOAT, false, 0, 0);
            if self.reset_divisor {
                gl.vertex_attrib_divisor(self.a_pos, 0);
            }
            gl.active_texture(glow::TEXTURE0);
            gl.uniform_1_i32(Some(&self.u_tex), 0);

            for quad in quads {
                let Some(tex_id) = NonZeroU32::new(quad.texture.tex_id()) else {
                    continue;
                };
                gl.bind_texture(glow::TEXTURE_2D, Some(glow::NativeTexture(tex_id)));
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                let mvp = view_proj * quad.model;
                gl.uniform_matrix_4_f32_slice(Some(&self.u_mvp), false, &mvp.to_cols_array());
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            }

            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.disable_vertex_attrib_array(self.a_pos);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.use_program(None);
        }
    }
}

/// A full-output element that draws the whole 3D scene (stage 2).
///
/// It reports a new commit every frame, so the damage tracker always redraws all of it.
#[derive(Debug)]
pub struct SceneElement {
    id: Id,
    commit: CommitCounter,
    size: Size<i32, Physical>,
    gl: SceneGl,
    view_proj: Mat4,
    quads: Vec<Quad>,
}

impl Element for SceneElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size((self.size.w as f64, self.size.h as f64).into())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::from_size(self.size)
    }
}

impl RenderElement<GlowRenderer> for SceneElement {
    fn draw(
        &self,
        frame: &mut GlowFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let view_proj = target_matrix(frame.transformation()) * self.view_proj;
        frame.with_context(|gl| unsafe { self.gl.draw(gl, view_proj, &self.quads) })
    }
}
