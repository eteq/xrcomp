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

use crate::state::{Backend, State};

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
                            } else {
                                FilterResult::Forward
                            }
                        } else {
                                FilterResult::Forward

                        }
                    },
                );

                if let Some(action) = action {
                    action.do_action(self);
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

enum CompositorAction {
    QuitFocusedWindow,
    QuitCompositor,
    ReloadConfig,
}

impl CompositorAction {
    fn do_action<B: Backend>(&self, state: &mut State<B>) {
        match self {
            CompositorAction::QuitFocusedWindow => { let _ = quit_focused_window(state); },
            CompositorAction::QuitCompositor => { let _ = quit_compositor(state); },
            CompositorAction::ReloadConfig => { let _ = state.reload_config(); }
        }
    }
}

fn quit_focused_window<B: Backend>(state: &mut State<B>) -> Result<(), ()> {
    if let Some(focused) = state.seat.get_keyboard().unwrap().current_focus() {
        if let Some(window) = state.space.elements().find(|w| w.toplevel().unwrap().wl_surface() == &focused) {
            if let Some(toplevel) = window.clone().toplevel() {
                toplevel.send_close();
                return Ok(())
            }
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