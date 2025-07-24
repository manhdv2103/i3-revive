use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::net::UnixStream;

use directories::BaseDirs;
use serde_json::{json, Value};

use crate::i3ipc::{get_workspaces, run_command};

fn get_metadata_path() -> io::Result<std::path::PathBuf> {
    let base_dirs = BaseDirs::new().expect("Failed to get base directories");
    let mut path = base_dirs.data_local_dir().to_path_buf();
    path.push("i3-revive");
    fs::create_dir_all(&path)?;
    path.push("metadata.json");
    Ok(path)
}

pub fn save_metadata(stream: &mut UnixStream) -> io::Result<()> {
    let workspaces = get_workspaces(stream).expect("Failed to get workspaces");
    let mut visible_workspaces = workspaces
        .as_array()
        .unwrap()
        .iter()
        .filter(|ws| {
            ws.get("visible")
                .and_then(|visible| visible.as_bool())
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    // We want the focused workspace to be last so it will be focused when restoring
    visible_workspaces.sort_by(|a, _| {
        if a.get("focused")
            .and_then(|focused| focused.as_bool())
            .unwrap_or(false)
        {
            Ordering::Greater
        } else {
            Ordering::Less
        }
    });

    let visible_workspace_names = visible_workspaces
        .iter()
        .map(|ws| ws.get("name").unwrap().as_str().unwrap())
        .collect::<Vec<_>>();

    let path = get_metadata_path()?;
    let mut file = File::create(path)?;
    let metadata = json!({
        "visible_workspaces": visible_workspace_names
    });
    file.write_all(serde_json::to_string_pretty(&metadata)?.as_bytes())?;
    Ok(())
}

pub fn restore_metadata(stream: &mut UnixStream) -> io::Result<()> {
    let path = get_metadata_path()?;
    if !path.exists() {
        eprintln!("Metadata file is missing");
        std::process::exit(1);
    }

    let workspaces = get_workspaces(stream).expect("Failed to get workspaces");
    let json_content = fs::read_to_string(&path).expect("Failed to read metadata.json file");
    let metadata: Value =
        serde_json::from_str(&json_content).expect("Failed to deserialize metadata.json");

    let restoring_workspaces = metadata.get("visible_workspaces").unwrap().as_array().unwrap();
    let focused_workspace_name = workspaces
        .as_array()
        .unwrap()
        .iter()
        .find(|ws| {
            ws.get("focused")
                .and_then(|focused| focused.as_bool())
                .unwrap_or(false)
        })
        .map(|ws| ws.get("name").unwrap().as_str().unwrap());

    let mut first_ws = true;
    for ws in restoring_workspaces {
        let ws_name = ws.as_str().unwrap();

        if !first_ws || focused_workspace_name.is_none_or(|name| name != ws_name) {
            run_command(stream, format!("workspace {}", ws_name).as_str()).unwrap();
        }
        
        first_ws = false;
    }

    Ok(())
}

pub fn remove_metadata() -> io::Result<()> {
    let path = get_metadata_path()?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}
