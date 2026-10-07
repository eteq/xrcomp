//! Shared rendering helpers used by all backends.
//!
//! xrcomp does not render a themed/client cursor image; it just draws a
//! small solid-colored square at the pointer location so there is visual
//! feedback of where the pointer is on backends that don't get
//! a cursor drawn for them by a host compositor.

use smithay::{
    backend::renderer::{
        Color32F,
        element::{Kind, solid::SolidColorRenderElement},
        glow::GlowRenderer,
    },
    output::Output,
    utils::{Physical, Point},
};

pub const CLEAR_COLOR: Color32F = Color32F::new(0.1, 0.1, 0.1, 1.0);

// The set of render elements used for all backends: the 3D scene (see `scene.rs`), which
// draws every window, plus our software cursor square. All backends only ever use
// `GlowRenderer`, so this is tied to that concrete type rather than being generic.
smithay::backend::renderer::element::render_elements! {
    pub OutputElement<=GlowRenderer>;
    Scene=crate::scene::SceneElement,
    Cursor=SolidColorRenderElement,
}

/// The elements for one output frame, front to back: the cursor on top of the scene.
pub fn output_elements(
    cursor: SolidColorRenderElement,
    scene: Option<crate::scene::SceneElement>,
) -> Vec<OutputElement> {
    std::iter::once(OutputElement::Cursor(cursor))
        .chain(scene.map(OutputElement::Scene))
        .collect()
}

pub struct Cursor {
    buffer: smithay::backend::renderer::element::solid::SolidColorBuffer,
}

impl Cursor {
    pub fn new() -> Self {
        Self {
            buffer: smithay::backend::renderer::element::solid::SolidColorBuffer::new(
                (10, 10),
                [0.9, 0.3, 0.1, 1.0],
            ),
        }
    }

    pub fn render_element(
        &self,
        location: Point<i32, Physical>,
        scale: impl Into<smithay::utils::Scale<f64>>,
    ) -> SolidColorRenderElement {
        SolidColorRenderElement::from_buffer(&self.buffer, location, scale, 1.0, Kind::Cursor)
    }
}

impl Default for Cursor {
    fn default() -> Self {
        Self::new()
    }
}

pub fn output_scale(output: &Output) -> smithay::utils::Scale<f64> {
    smithay::utils::Scale::from(output.current_scale().fractional_scale())
}
