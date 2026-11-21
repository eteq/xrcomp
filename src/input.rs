use std::time::Instant;

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    }, input::{
        keyboard::{FilterResult, Keysym},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::SERIAL_COUNTER,
};

use glam::Mat4;

use crate::state::{Backend, State, OutputKey};
use xrcomp::ipc::CompositorAction;

impl<BackendData: Backend + 'static> State<BackendData> {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time(&event);
                let press_state = event.state();

                let action = self.seat.get_keyboard().unwrap().input::<CompositorAction, _>(
                    self,
                    event.key_code(),
                    press_state,
                    serial,
                    time,
                    |state, modifiers, handle| {
                        if press_state == KeyState::Pressed && state.config.compositor_modifier.matches_state(modifiers) {
                            if handle.raw_syms().contains(&Keysym::from_char(state.config.quit_focused_window_key)) {
                                FilterResult::Intercept(CompositorAction::QuitFocusedWindow)
                            } else if handle.raw_syms().contains(&Keysym::from_char(state.config.quit_compositor_key)) {
                                FilterResult::Intercept(CompositorAction::QuitCompositor)
                            } else if handle.raw_syms().contains(&Keysym::from_char(state.config.reload_config_key)) {
                                FilterResult::Intercept(CompositorAction::ReloadConfig)
                            } else if handle.raw_syms().contains(&Keysym::from_char(state.config.pan_left_key)) {
                                FilterResult::Intercept(CompositorAction::PanLeft)
                            } else if handle.raw_syms().contains(&Keysym::from_char(state.config.pan_right_key)) {
                                FilterResult::Intercept(CompositorAction::PanRight)
                            } else if handle.raw_syms().contains(&Keysym::from_char(state.config.pan_up_key)) {
                                FilterResult::Intercept(CompositorAction::PanUp)
                            } else if handle.raw_syms().contains(&Keysym::from_char(state.config.pan_down_key)) {
                                FilterResult::Intercept(CompositorAction::PanDown)
                            } else {
                                FilterResult::Forward
                            }
                        } else {
                                FilterResult::Forward

                        }
                    },
                );

                if let Some(action) = action {
                    let _ = do_compositor_action(&action, self);
                }
            }
            // Relative motion: emitted by real pointer devices (mice, trackpads) through
            // libinput on the udev backend. There is no absolute position, so we accumulate
            // a delta on top of the pointer's current location.
            InputEvent::PointerMotion { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();

                let mut location = self.clamp_coords(pointer.current_location() + event.delta());
                location = self.clamp_coords(location);

                let serial = SERIAL_COUNTER.next_serial();
                let under = self.surface_under(location);

                pointer.relative_motion(
                    self,
                    under.clone(),
                    &RelativeMotionEvent {
                        delta: event.delta(),
                        delta_unaccel: event.delta_unaccel(),
                        time: event.time(),
                    },
                );
                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location,
                        serial,
                        time: event.time(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let output = self.space.outputs().next().unwrap();

                let output_geo = self.space.output_geometry(output).unwrap();

                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();

                let serial = SERIAL_COUNTER.next_serial();

                let pointer = self.seat.get_pointer().unwrap();

                let under = self.surface_under(pos);

                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location: pos,
                        serial,
                        time: event.time(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let keyboard = self.seat.get_keyboard().unwrap();

                let serial = SERIAL_COUNTER.next_serial();

                let button = event.button_code();

                let button_state = event.state();

                if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
                    if let Some((window, _loc)) = self
                        .space
                        .element_under(pointer.current_location())
                        .map(|(w, l)| (w.clone(), l))
                    {
                        self.space.raise_element(&window, true);
                        keyboard.set_focus(
                            self,
                            Some(window.toplevel().unwrap().wl_surface().clone()),
                            serial,
                        );
                        self.space.elements().for_each(|window| {
                            window.toplevel().unwrap().send_pending_configure();
                        });
                    } else {
                        self.space.elements().for_each(|window| {
                            window.set_activated(false);
                            window.toplevel().unwrap().send_pending_configure();
                        });
                        keyboard.set_focus(self, Option::<WlSurface>::None, serial);
                    }
                };

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();

                let horizontal_amount = event
                    .amount(Axis::Horizontal)
                    .unwrap_or_else(|| event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.);
                let vertical_amount = event
                    .amount(Axis::Vertical)
                    .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.);
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

                let mut frame = AxisFrame::new(event.time()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal_amount);
                    if let Some(discrete) = horizontal_amount_discrete {
                        frame = frame.v120(Axis::Horizontal, discrete as i32);
                    }
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical_amount);
                    if let Some(discrete) = vertical_amount_discrete {
                        frame = frame.v120(Axis::Vertical, discrete as i32);
                    }
                }

                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }
    }
}

pub fn do_compositor_action<B: Backend>(action: &CompositorAction, state: &mut State<B>) -> Result<(), ()> {
    match action {
        CompositorAction::QuitFocusedWindow => { quit_focused_window(state) },
        CompositorAction::QuitCompositor => { quit_compositor(state) },
        CompositorAction::ReloadConfig => { 
            let res = state.reload_config();
            if res.is_ok() { Ok(()) } else { Err(()) }  
         },
        CompositorAction::PanLeft => { pan_camera_vp(state, PanDirection::Left, state.config.pan_step_deg) },
        CompositorAction::PanRight => { pan_camera_vp(state, PanDirection::Right, state.config.pan_step_deg) },
        CompositorAction::PanUp => { pan_camera_vp(state, PanDirection::Up, state.config.pan_step_deg) },
        CompositorAction::PanDown => { pan_camera_vp(state, PanDirection::Down, state.config.pan_step_deg) },
    }
}


fn quit_focused_window<B: Backend>(state: &mut State<B>) -> Result<(), ()> {
    if let Some(foucused) = state.get_focused_window() {
        if let Some(toplevel) = foucused.toplevel() {
            toplevel.send_close();
            return Ok(());
        }
    }
    Err(())
}

fn quit_compositor<B: Backend>(state: &mut State<B>) -> Result<(), ()> {
    if state.config.quit_compositor_key_timeout_ms == 0 {
        state.running = false;
        return Ok(());
    }

    if let Some(lastpress) = state.quit_key_lastpress {
        if lastpress.elapsed().as_millis() < state.config.quit_compositor_key_timeout_ms as u128 {
            state.running = false;
        }
    }

    state.quit_key_lastpress = Some(Instant::now());

    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub enum PanDirection {
    Left,
    Right,
    Up,
    Down,
}

fn pan_camera_vp<B: Backend>(state: &mut State<B>, direction: PanDirection, degs: f32) -> Result<(), ()> {
    if let Some(output) = state.get_focused_output() {
        let key = OutputKey::from(&output);
        if let Some(camera) = state.outputs_camera_vp.remove(&key) {
            let rads = degs.to_radians();
            let panmat = match direction {
                PanDirection::Left => { Mat4::from_rotation_y(rads) },
                PanDirection::Right => { Mat4::from_rotation_y(-rads) },
                PanDirection::Up => { Mat4::from_rotation_x(rads) },
                PanDirection::Down => { Mat4::from_rotation_x(-rads) },
            };
            let (v, p) = camera;
            state.outputs_camera_vp.insert(key, (panmat * v, p));
            return Ok(());
        }
    }
    Err(())
}