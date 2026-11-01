use std::sync::{LazyLock, RwLock};

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

pub static CONFIG: LazyLock<RwLock<XrcompConfig>> = LazyLock::new(|| {
    RwLock::new(confy::load("xrcomp", None).expect("Failed to load configuration"))
});

pub fn write_current_conf() -> Result<(), confy::ConfyError> {
    let config = CONFIG.read().unwrap();
    confy::store("xrcomp", None, &*config)
}

pub fn get_conf_path() -> Result<std::path::PathBuf, confy::ConfyError> {
    confy::get_configuration_file_path("xrcomp", None)
}   