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
    // TODO: allow to pass in command args to command mapping
    pub window_command_mappings: Vec<WindowCommandMapping>,
    pub window_swallow_criteria: HashMap<String, HashSet<String>>,
    pub terminal_allow_revive_processes: HashSet<String>,
    // TODO: allow to pass in terminal args to the revive command
    pub terminal_revive_commands: HashMap<String, String>,
}

pub static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn load_config() {
    let default_config = Config {
        window_command_mappings: vec![],
        window_swallow_criteria: HashMap::new(),
        terminal_allow_revive_processes: HashSet::new(),
        terminal_revive_commands: HashMap::new(),
    };
    
    if let Some(base_dirs) = BaseDirs::new() {
        let config_path = base_dirs.config_dir().join("i3-revive/config.json");
        let config = match fs::read_to_string(config_path) {
            Ok(content) => serde_json::from_str(&content).unwrap(),
            Err(_) => default_config,
        };
        CONFIG.set(config).unwrap();
    } else {
        CONFIG
            .set(default_config)
            .unwrap();
    }
}
