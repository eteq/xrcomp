#![allow(dead_code)]

use smithay::desktop::Window;
use glam::{Vec3, Mat4};
use glam_newer;

// These helpers serve two purposes: 1) defaulting to opengl conventions, 2) interface these functions that are new in glam 0.34 to smithay's required glam
fn look_at(position: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let positionn = glam_newer::Vec3::from_array(position.to_array());
    let targetn = glam_newer::Vec3::from_array(target.to_array());
    let upn = glam_newer::Vec3::from_array(up.to_array());
    let newmat4 = glam_newer::camera::model::look_at_mat4(positionn, targetn, upn,
        glam_newer::Vec3::NEG_Z, glam_newer::Vec3::Y);

    Mat4::from_cols_array(&newmat4.to_cols_array())
}
#[allow(dead_code)]
fn look_to(position: Vec3, dir: Vec3, up: Vec3) -> Mat4 {
    let positionn = glam_newer::Vec3::from_array(position.to_array());
    let dirn = glam_newer::Vec3::from_array(dir.to_array());
    let upn = glam_newer::Vec3::from_array(up.to_array());
    let newmat4 = glam_newer::camera::model::look_to_mat4(positionn, dirn, upn,
        glam_newer::Vec3::NEG_Z, glam_newer::Vec3::Y);

    Mat4::from_cols_array(&newmat4.to_cols_array())
}

/// The position and orientation of a window in 3D space.
/// target/up are options because they are assumed to be the origin and Y if not given (the usual case)
#[derive(Default, Clone, Copy, Debug)]
pub struct WindowPose {
    pub position: Vec3,
    pub target: Option<Vec3>,
    pub up: Option<Vec3>,
    cached_mat: Option<Mat4>
}

impl WindowPose {
    pub fn new(position: Vec3, target: Option<Vec3>, up: Option<Vec3>) -> Self {
        Self {
            position,
            target,
            up,
            cached_mat: None,
        }
    }

    pub fn make_mat4(&mut self) -> Mat4 {
        if let Some(mat) = self.cached_mat {
            mat
        } else {
            let target = self.target.unwrap_or(Vec3::ZERO);
            let up = self.up.unwrap_or(Vec3::Y);
            let mat = look_at(self.position, target, up);
            self.cached_mat = Some(mat);
            mat
        }
    }
}

pub fn yaw_view(view: Mat4, yawradians: f32) -> Mat4 {
    view * Mat4::from_rotation_y(-yawradians)
}
pub fn pitch_view(view: Mat4, pitchradians: f32) -> Mat4 {
    view * Mat4::from_rotation_x(pitchradians)
}

// The below is old/temporary, the above is the intended implementation for longer-term.

// Where each window sits in the 3D scene.
//
// This is compositor state rather than rendering state: it is independent of any backend
// or renderer, and is what input hit-testing, IPC etc. should consult once windows leave
// the z = 0 plane. The GPU side of the scene (textures, GL objects) lives in
// [`crate::scene::SceneRenderer`], which reads placements from here.
//
// The placement is stored in the window's own user data, so it is created on first use
// and dropped together with the window; there is no separate map to keep in sync with
// the `Space`.


/// Per-window placement, stored in [`Window::user_data`].
struct WindowPlacement(std::cell::Cell<Mat4>);

fn placement_cell(window: &Window) -> &std::cell::Cell<Mat4> {
    let user_data = window.user_data();
    user_data.insert_if_missing(|| WindowPlacement(std::cell::Cell::new(Mat4::IDENTITY)));
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