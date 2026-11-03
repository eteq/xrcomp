use confy;
use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct XrcompConfig {
    pub compositor_modifier: ModifierKey,
    pub quit_focused_window_key: char,
    pub quit_compositor_key: char,
    pub quit_compositor_key_timeout_ms: u32,
}

impl Default for XrcompConfig {
    fn default() -> Self {
        Self {
            compositor_modifier: ModifierKey::Super,
            quit_focused_window_key: 'w',
            quit_compositor_key: 'x',
            quit_compositor_key_timeout_ms: 1000,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ModifierKey {
    Ctrl,
    Alt,
    Shift,
    Super,
    Nomod
}

impl  ModifierKey {
    pub fn matches_state(&self, modifiers: &smithay::input::keyboard::ModifiersState) -> bool {
        match self {
            ModifierKey::Ctrl => modifiers.ctrl && !modifiers.alt && !modifiers.shift && !modifiers.logo,
            ModifierKey::Alt => !modifiers.ctrl && modifiers.alt && !modifiers.shift && !modifiers.logo,
            ModifierKey::Shift => !modifiers.ctrl && !modifiers.alt && modifiers.shift && !modifiers.logo,
            ModifierKey::Super => !modifiers.ctrl && !modifiers.alt && !modifiers.shift && modifiers.logo,
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