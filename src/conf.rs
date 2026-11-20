use confy;
use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct XrcompConfig {
    pub compositor_modifier: ModifierKey,
    pub quit_focused_window_key: char,
    pub reload_config_key: char,
    pub quit_compositor_key: char,
    pub quit_compositor_key_timeout_ms: u32,
    pub pan_left_key: char,
    pub pan_right_key: char,
    pub pan_up_key: char,
    pub pan_down_key: char,
    pub pan_step_deg: f32,
    pub cursor_size: u32,
    pub cursor_color: String,
}

impl Default for XrcompConfig {
    fn default() -> Self {
        Self {
            compositor_modifier: ModifierKey::Super,
            quit_focused_window_key: 'q',
            reload_config_key: 'r',
            quit_compositor_key: 'x',
            pan_left_key: 'a',
            pan_right_key: 'd',
            pan_up_key: 'w',
            pan_down_key: 's',
            pan_step_deg: 1.0,
            quit_compositor_key_timeout_ms: 1000,
            cursor_size: 10,
            cursor_color: String::from("#E64C1AFF"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ModifierKey {
    Ctrl,
    Alt,
    Shift,
    Super,
    CtrlSuper,
    AltSuper,
    ShiftSuper,
    Nomod
}

impl  ModifierKey {
    pub fn matches_state(&self, modifiers: &smithay::input::keyboard::ModifiersState) -> bool {
        match self {
            ModifierKey::Ctrl => modifiers.ctrl && !modifiers.alt && !modifiers.shift && !modifiers.logo,
            ModifierKey::Alt => !modifiers.ctrl && modifiers.alt && !modifiers.shift && !modifiers.logo,
            ModifierKey::Shift => !modifiers.ctrl && !modifiers.alt && modifiers.shift && !modifiers.logo,
            ModifierKey::Super => !modifiers.ctrl && !modifiers.alt && !modifiers.shift && modifiers.logo,
            ModifierKey::CtrlSuper => modifiers.ctrl && !modifiers.alt && !modifiers.shift && modifiers.logo,
            ModifierKey::AltSuper => !modifiers.ctrl && modifiers.alt && !modifiers.shift && modifiers.logo,
            ModifierKey::ShiftSuper => !modifiers.ctrl && !modifiers.alt && modifiers.shift && modifiers.logo,
            ModifierKey::Nomod => !modifiers.ctrl && !modifiers.alt && !modifiers.shift && !modifiers.logo,
        }
    }
}

pub fn load_config() -> Result<XrcompConfig, confy::ConfyError> {
    confy::load("xrcomp", None)
}

pub fn write_config(config: &XrcompConfig) -> Result<(), confy::ConfyError> {
    confy::store("xrcomp", None, &config)
}

pub fn get_config_path() -> Result<std::path::PathBuf, confy::ConfyError> {
    confy::get_configuration_file_path("xrcomp", None)
}   