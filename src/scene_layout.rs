//! Where each window sits in the 3D scene.
//!
//! This is compositor state rather than rendering state: it is independent of any backend
//! or renderer, and is what input hit-testing, IPC etc. should consult once windows leave
//! the z = 0 plane. The GPU side of the scene (textures, GL objects) lives in
//! [`crate::scene::SceneRenderer`], which reads placements from here.
//!
//! The placement is stored in the window's own user data, so it is created on first use
//! and dropped together with the window; there is no separate map to keep in sync with
//! the `Space`.

use std::cell::Cell;

use glam::Mat4;
use smithay::desktop::Window;

/// Per-window placement, stored in [`Window::user_data`].
struct WindowPlacement(Cell<Mat4>);

fn placement_cell(window: &Window) -> &Cell<Mat4> {
    let user_data = window.user_data();
    user_data.insert_if_missing(|| WindowPlacement(Cell::new(Mat4::IDENTITY)));
    &user_data.get::<WindowPlacement>().unwrap().0
}

/// Where a window sits in 3D, applied on top of its 2D layout position.
///
/// The identity (the default) keeps the window on the z = 0 plane, where it lines up
/// exactly with its 2D position in the `Space`.
pub fn window_placement(window: &Window) -> Mat4 {
    placement_cell(window).get()
}

/// Set a window's 3D placement (see [`window_placement`]).
#[allow(dead_code)] // Nothing moves windows off the z = 0 plane yet.
pub fn set_window_placement(window: &Window, placement: Mat4) {
    placement_cell(window).set(placement);
}
