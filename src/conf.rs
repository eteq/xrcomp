use confy;
use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct XrcompConfig {
    compositor_modifier: ModifierKey,
}

impl Default for XrcompConfig {
    fn default() -> Self {
        Self {
            compositor_modifier: ModifierKey::Super,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ModifierKey {
    Ctrl,
    Alt,
    Shift,
    Super,
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