use directories::BaseDirs;
use serde::Deserialize;
use std::fs;
use std::sync::OnceLock;

#[derive(Deserialize, Debug)]
pub struct WindowCommandMapping {
    pub regex: String,
    pub command: String,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    #[serde(default)]
    pub window_command_mappings: Vec<WindowCommandMapping>,
}

pub static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn load_config() {
    if let Some(base_dirs) = BaseDirs::new() {
        let config_path = base_dirs.config_dir().join("i3-revive/config.json");
        let config = match fs::read_to_string(config_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|_| Config {
                window_command_mappings: vec![],
            }),
            Err(_) => Config {
                window_command_mappings: vec![],
            },
        };
        CONFIG.set(config).unwrap();
    } else {
        CONFIG
            .set(Config {
                window_command_mappings: vec![],
            })
            .unwrap();
    }
}