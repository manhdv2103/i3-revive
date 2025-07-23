use directories::BaseDirs;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::OnceLock;

#[derive(Deserialize, Debug)]
pub struct WindowCommandMapping {
    pub class: Option<String>,
    pub title: Option<String>,
    pub command: Option<String>,
    pub working_directory: Option<String>,
    pub once: Option<bool>,
    pub ignored: Option<bool>,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    #[serde(default)]
    pub window_command_mappings: Vec<WindowCommandMapping>,
    pub window_swallow_criteria: HashMap<String, HashSet<String>>,
}

pub static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn load_config() {
    if let Some(base_dirs) = BaseDirs::new() {
        let config_path = base_dirs.config_dir().join("i3-revive/config.json");
        let config = match fs::read_to_string(config_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|_| Config {
                window_command_mappings: vec![],
                window_swallow_criteria: HashMap::new(),
            }),
            Err(_) => Config {
                window_command_mappings: vec![],
                window_swallow_criteria: HashMap::new(),
            },
        };
        CONFIG.set(config).unwrap();
    } else {
        CONFIG
            .set(Config {
                window_command_mappings: vec![],
                window_swallow_criteria: HashMap::new(),
            })
            .unwrap();
    }
}
