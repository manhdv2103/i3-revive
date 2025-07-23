use crate::config::{WindowCommandMapping, CONFIG};
use crate::i3_tree;
use directories::BaseDirs;
use regex::Regex;
use serde::{Deserialize, Serialize};
use shlex::split;
use std::collections::HashSet;
use std::{fs, io, os::unix::fs::PermissionsExt, path::Path};
use xcb::{x, XidNew};

#[derive(Serialize, Deserialize, Debug)]
struct Process {
    command: Vec<String>,
    working_directory: String,
}

fn get_pid(window: u32) -> xcb::Result<u32> {
    let (conn, _) = xcb::Connection::connect(None)?;

    let wm_pid = conn.send_request(&x::InternAtom {
        only_if_exists: true,
        name: "_NET_WM_PID".as_bytes(),
    });
    let wm_pid = conn.wait_for_reply(wm_pid)?.atom();
    assert!(wm_pid != x::ATOM_NONE, "_NET_WM_PID is not supported");

    let cookie = conn.send_request(&x::GetProperty {
        delete: false,
        window: unsafe { XidNew::new(window) },
        property: wm_pid,
        r#type: x::ATOM_CARDINAL,
        long_offset: 0,
        long_length: 1024,
    });
    let reply = conn.wait_for_reply(cookie)?;
    let pid = reply.value::<u32>()[0];

    Ok(pid)
}

// https://docs.rs/is_executable/latest/src/is_executable/lib.rs.html#38
fn is_executable(path: &Path) -> bool {
    let metadata = match path.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return false,
    };
    let permissions = metadata.permissions();
    metadata.is_file() && permissions.mode() & 0o111 != 0
}

// https://github.com/giampaolo/psutil/issues/1179
fn get_process_cmd(pid: u32) -> Result<Vec<String>, String> {
    let raw_cmd = match fs::read_to_string(format!("/proc/{}/cmdline", pid)) {
        Ok(cmd) => cmd,
        Err(err) => return Err(err.to_string()),
    };

    let cmd_parts = raw_cmd
        .trim_matches('\0')
        .split('\0')
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    if cmd_parts.len() != 1 {
        return Ok(cmd_parts);
    }

    // Process may use space (" ") as a separator
    let raw_cmd = cmd_parts.first().unwrap();

    // Process's executable name may have space (" "), so we just have to try
    // every combinations until finding an existing executable
    let mut executable = String::new();
    let mut parts_iter = raw_cmd.split(' ');
    for part in parts_iter.by_ref() {
        if !executable.is_empty() {
            executable.push(' ');
        }
        executable.push_str(part);

        let path = Path::new(&executable);
        if path.exists() && path.is_file() && is_executable(path) {
            break;
        }
    }
    let mut cmd_parts = parts_iter.map(|s| s.to_string()).collect::<Vec<_>>();
    cmd_parts.insert(0, executable);

    Ok(cmd_parts)
}

fn get_process_cwd(pid: u32, is_terminal: bool) -> io::Result<String> {
    let mut path = fs::read_link(format!("/proc/{}/cwd", pid))?;
    if is_terminal {
        // If the program is a terminal emulator, get the working
        // directory from its first subprocess.
        if let Some(first_child_pid) = fs::read_to_string(format!("/proc/{}/task/{}/children", pid, pid))?.split_whitespace().next() {
            path = fs::read_link(format!("/proc/{}/cwd", first_child_pid))?;
        }
    }

    Ok(path.into_os_string().into_string().unwrap())
}

pub fn save_processes(windows: Vec<i3_tree::Window>) {
    let config = CONFIG.get().unwrap();
    let base_dirs = BaseDirs::new().expect("Failed to get base directories");
    let mut once_mappings = HashSet::new();
    let processes = windows
        .iter()
        .filter_map(|w| {
            let mut command: Option<Vec<String>> = None;
            let mut working_directory: Option<String> = None;
            let mut matched_mapping: Option<&WindowCommandMapping> = None;
            let mut matched_mapping_idx: Option<usize> = None;
            let mut best_score = 0;
            for (i, mapping) in config.window_command_mappings.iter().enumerate() {
                let title_regex = &mapping
                    .title
                    .as_ref()
                    .map(|str| Regex::new(str.as_str()).unwrap());
                let class_regex = &mapping
                    .class
                    .as_ref()
                    .map(|str| Regex::new(str.as_str()).unwrap());

                let mut score = 0;
                if title_regex.as_ref().is_some_and(|re| re.is_match(&w.name)) {
                    score += 2;
                }
                if class_regex.as_ref().is_some_and(|re| re.is_match(&w.class)) {
                    score += 1;
                }

                if score > best_score {
                    best_score = score;
                    matched_mapping = Some(mapping);
                    matched_mapping_idx = Some(i);
                }
            }

            if let Some(mapping) = matched_mapping {
                if mapping.ignored.is_some_and(|ignored| ignored) {
                    return None;
                }

                if let Some(mapping_idx) = matched_mapping_idx {
                    if mapping.once.is_some_and(|once| once) {
                        if once_mappings.contains(&mapping_idx) {
                            return None;
                        }

                        once_mappings.insert(mapping_idx);
                    }
                }

                if let Some(command_str) = &mapping.command {
                    command = split(command_str);
                }
                if let Some(working_directory_str) = &mapping.working_directory {
                    working_directory = Some(working_directory_str.clone());
                }
            }

            let pid = get_pid(w.id).unwrap();
            Some(Process {
                command: command.unwrap_or_else(|| get_process_cmd(pid).unwrap()),
                working_directory: working_directory.unwrap_or_else(|| {
                    get_process_cwd(pid, config.terminals.contains(&w.class)).unwrap()
                }),
            })
        })
        .collect::<Vec<_>>();

    let json = serde_json::to_string_pretty(&processes).expect("Failed to serialize");

    let mut dir = base_dirs.data_local_dir().to_path_buf();
    dir.push("i3-revive");
    fs::create_dir_all(&dir).expect("Failed to create directory");

    let mut file_path = dir.clone();
    file_path.push("processes.json");
    fs::write(file_path, json).expect("Failed to write file");
}
