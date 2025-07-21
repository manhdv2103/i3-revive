use std::{fs, io, os::unix::fs::PermissionsExt, path::Path};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use xcb::{x, XidNew};

#[derive(Serialize, Deserialize, Debug)]
struct Process {
    command: Vec<String>,
    working_directory: String,
}

fn get_pids(windows: &Vec<u32>) -> xcb::Result<Vec<u32>> {
    let (conn, _) = xcb::Connection::connect(None)?;

    let wm_pid = conn.send_request(&x::InternAtom {
        only_if_exists: true,
        name: "_NET_WM_PID".as_bytes(),
    });
    let wm_pid = conn.wait_for_reply(wm_pid)?.atom();
    assert!(wm_pid != x::ATOM_NONE, "_NET_WM_PID is not supported");

    let mut res: Vec<u32> = vec![];

    for window in windows {
        let cookie = conn.send_request(&x::GetProperty {
            delete: false,
            window: unsafe { XidNew::new(*window) },
            property: wm_pid,
            r#type: x::ATOM_CARDINAL,
            long_offset: 0,
            long_length: 1024,
        });
        let reply = conn.wait_for_reply(cookie)?;
        let pid = reply.value::<u32>()[0];

        res.push(pid);
    }

    Ok(res)
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

fn get_process_cwd(pid: u32) -> io::Result<String> {
    let path = fs::read_link(format!("/proc/{}/cwd", pid))?;
    Ok(path.into_os_string().into_string().unwrap())
}

pub fn save_processes(windows: Vec<u32>) {
    let pids = get_pids(&windows).unwrap();
    let processes = pids
        .iter()
        .map(|&pid| Process {
            command: get_process_cmd(pid).unwrap(),
            working_directory: get_process_cwd(pid).unwrap(),
        })
        .collect::<Vec<_>>();

    let json = serde_json::to_string_pretty(&processes).expect("Failed to serialize");

    let base_dirs = BaseDirs::new().expect("Failed to get base directories");
    let mut dir = base_dirs.data_local_dir().to_path_buf();
    dir.push("i3-revive");
    fs::create_dir_all(&dir).expect("Failed to create directory");

    let mut file_path = dir.clone();
    file_path.push("processes");
    fs::write(file_path, json).expect("Failed to write file");
}