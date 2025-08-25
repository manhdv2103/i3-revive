use crate::config::{TerminalCommandMapping, WindowCommandMapping, CONFIG};
use crate::i3_tree;
use chrono::Local;
use directories::BaseDirs;
use regex::Regex;
use serde::{Deserialize, Serialize};
use shlex::{split, try_join};
use std::collections::HashSet;
use std::error::Error;
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::{fs, io, os::unix::fs::PermissionsExt, path::Path};
use xcb::{x, XidNew};

#[derive(Serialize, Deserialize, Debug)]
struct Process {
    command: Vec<String>,
    working_directory: String,
}

fn get_pid(window: u32) -> Result<u32, Box<dyn Error>> {
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
    let value = reply.value::<u32>();
    if value.is_empty() {
        Err("Window has no pid".into())
    } else {
        Ok(value[0])
    }
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

fn get_terminal_process_cmd(
    pid: u32,
    window_id: u32,
    terminal_command: String,
) -> Result<Vec<String>, String> {
    let config = CONFIG.get().unwrap();
    let shell_pid = fs::read_to_string(format!("/proc/{}/task/{}/children", pid, pid))
        .map_err(|err| err.to_string())?
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned();

    let raw_shell_cmd = match fs::read_to_string(format!("/proc/{}/cmdline", shell_pid)) {
        Ok(cmd) => cmd,
        Err(err) => return Err(err.to_string()),
    };
    let shell_cmd_parts = raw_shell_cmd
        .trim_matches('\0')
        .split('\0')
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    if shell_cmd_parts.is_empty() {
        return Err("Shell command not found".into());
    }

    let shell_cmd = shell_cmd_parts.first().unwrap();
    let shell_name = shell_cmd
        .rsplit_once('/')
        .map(|parts| parts.1)
        .unwrap_or(shell_cmd);
    let get_process_cmd_parts = || {
        if shell_name != "bash" && shell_name != "zsh" && shell_name != "fish" && shell_name != "sh"
        {
            eprintln!(
                "Warning: unknown shell or not a shell: {}, fallback to normal revival",
                shell_name
            );
            return Ok(None);
        }

        let is_revived_shell = shell_cmd_parts.get(1).is_some_and(|f| f == "-c")
            && shell_cmd_parts
                .get(2)
                .is_some_and(|a| a.contains("Revive-Terminal-Window-"));
        if shell_cmd_parts.len() > 1 && !is_revived_shell {
            eprintln!("Warning: shell has additional arguments, fallback to normal revival");
            return Ok(None);
        }

        let window_id_re = Regex::new("Revive-Terminal-Window-(\\d+)").unwrap();
        let revived_shell_window_id = if is_revived_shell {
            &window_id_re
                .captures(shell_cmd_parts.get(2).unwrap())
                .unwrap()[1]
        } else {
            ""
        };

        let fg_process_pid = if is_revived_shell
            && !Path::new(&format!("/tmp/i3-revive/{}", revived_shell_window_id)).exists()
        {
            // the "else" method of getting fd process id doesn't work for process that runs before the interactive shell
            // so we have to base on a flag (set below) to get the correct process id using a different method
            match fs::read_to_string(format!("/proc/{}/task/{}/children", shell_pid, shell_pid)) {
                Ok(children) => children
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .parse::<u32>()
                    .unwrap(),
                Err(err) => return Err(err.to_string()),
            }
        } else {
            match fs::read_to_string(format!("/proc/{}/stat", shell_pid)) {
                Ok(stat) => stat[(stat.rfind(')').unwrap() + 1)..]
                    .split_whitespace()
                    .nth(5)
                    .unwrap()
                    .parse::<u32>()
                    .unwrap(),
                Err(err) => return Err(err.to_string()),
            }
        };

        // no foreground process is running
        if fg_process_pid == shell_pid.parse::<u32>().unwrap() {
            return Ok(None);
        };

        let mut fg_process_cmd = match get_process_cmd(fg_process_pid) {
            Ok(cmd) => cmd,
            Err(e) => {
                eprintln!("Warning: cannot get process command of {} from terminal with pid of {}: {}", fg_process_pid, pid, e);
                return Ok(None);
            }
        };

        if fg_process_cmd
            .first()
            .map(|proc| config.terminal_allow_revive_processes.contains(proc))
            .unwrap_or(true)
        {
            let (program, args) = fg_process_cmd.split_first().unwrap();
            let args_str = try_join(args.iter().map(|a| a.as_str())).unwrap();

            let mut best_score = 0;
            let mut matched_mapping: Option<&TerminalCommandMapping> = None;

            for mapping in &config.terminal_command_mappings {
                let mut score = 0;
                if let Some(re_str) = &mapping.name {
                    if Regex::new(re_str).unwrap().is_match(program) {
                        score += 2;
                    } else {
                        continue;
                    }
                }
                if let Some(re_str) = &mapping.args {
                    if Regex::new(re_str).unwrap().is_match(&args_str) {
                        score += 1;
                    } else {
                        continue;
                    }
                }

                if score > best_score {
                    best_score = score;
                    matched_mapping = Some(mapping);
                }
            }

            if let Some(mapping) = matched_mapping {
                if let Some(command_str) = &mapping.command {
                    let re = Regex::new(r"\{(\d+)\}").unwrap();
                    let interpolated_command = re.replace_all(command_str, |caps: &regex::Captures| {
                        let index: usize = caps[1].parse().unwrap();
                        fg_process_cmd
                            .get(index)
                            .map_or("".to_string(), |s| shlex::try_quote(s).unwrap().to_string())
                    });
                    fg_process_cmd = split(&interpolated_command).unwrap();
                }
            }

            return Ok(Some(fg_process_cmd));
        }

        Ok(None)
    };
    // running process cmd inside an interactive shell let it to be run like if we run it manually
    let process_cmd = format!(
        "{}; mkdir /tmp/i3-revive > /dev/null; touch /tmp/i3-revive/{} > /dev/null",
        get_process_cmd_parts()?
            .map(|parts| { try_join(parts.iter().map(|s| s.as_str())).unwrap() })
            .unwrap_or("true".to_string()),
        window_id
    );
    let interactive_cmd_parts = [shell_cmd, "-i", "-c", process_cmd.as_str()];

    // the title echo should be run on non-interactive shell, as the shell can mess with the window title in interactive mode
    // the 0.5s sleep helps the title be displayed long enough to be detected by the swallow window
    let process_with_title_cmd = format!(
        "echo -ne \"\\033]0;Revive-Terminal-Window-{}\\007\"; sleep 0.5; {}; exec {}",
        window_id,
        &try_join(interactive_cmd_parts).unwrap(),
        shell_cmd
    );
    let cmd_parts = [shell_cmd, "-c", process_with_title_cmd.as_str()];

    let terminal_cmd_parts =
        split(&terminal_command.replace("{cmd}", &try_join(cmd_parts).unwrap()))
            .unwrap_or_else(|| panic!("Invalid terminal command: {}", terminal_command));

    Ok(terminal_cmd_parts)
}

fn get_process_cwd(pid: u32, is_terminal: bool) -> io::Result<String> {
    let mut path = fs::read_link(format!("/proc/{}/cwd", pid))?;
    if is_terminal {
        // If the program is a terminal emulator, get the working
        // directory from its first subprocess.
        if let Some(first_child_pid) =
            fs::read_to_string(format!("/proc/{}/task/{}/children", pid, pid))?
                .split_whitespace()
                .next()
        {
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
            if w.is_placeholder {
                return None;
            }

            let maybe_pid = get_pid(w.id);
            if let Err(err) = maybe_pid {
                eprintln!("Warning: Cannot found pid of window {}: {:?}", w.id, err);
                return None;
            }
            let pid = maybe_pid.unwrap();

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
                if let Some(re) = title_regex.as_ref() {
                    if re.is_match(&w.name) {
                        score += 2;
                    } else {
                        continue;
                    }
                }
                if let Some(re) = class_regex.as_ref() {
                    if w.class.as_deref().map(|class| re.is_match(class)).unwrap_or(false) {
                        score += 1;
                    } else {
                        continue;
                    }
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
                    let re = Regex::new(r"\{(\d+)\}").unwrap();
                    let interpolated_command = if re.is_match(command_str) {
                        get_process_cmd(pid).ok().map(|original_cmd_parts| {
                            re.replace_all(command_str, |caps: &regex::Captures| {
                                let index: usize = caps[1].parse().unwrap();
                                original_cmd_parts
                                    .get(index)
                                    .map_or("".to_string(), |s| shlex::try_quote(s).unwrap().to_string())
                            })
                            .into_owned()
                        })
                    } else {
                        None
                    };
                    command = split(interpolated_command.as_deref().unwrap_or(command_str));
                }
                if let Some(working_directory_str) = &mapping.working_directory {
                    working_directory = Some(working_directory_str.clone());
                }
            }

            let terminal_command = w
                .class
                .as_ref()
                .and_then(|class| config.terminal_revive_commands.get(class));

            Some(Process {
                command: command.unwrap_or_else(|| {
                    terminal_command
                        .map(|cmd| get_terminal_process_cmd(pid, w.id, cmd.to_string()).unwrap())
                        .unwrap_or_else(|| get_process_cmd(pid).unwrap())
                }),
                working_directory: working_directory
                    .unwrap_or_else(|| get_process_cwd(pid, terminal_command.is_some()).unwrap()),
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

pub fn restore_processes() {
    let base_dirs = BaseDirs::new().expect("Failed to get base directories");
    let mut file_path = base_dirs.data_local_dir().to_path_buf();
    file_path.push("i3-revive");
    file_path.push("processes.json");

    if !file_path.exists() {
        eprintln!("process file is missing");
        std::process::exit(1);
    }

    let json_content = fs::read_to_string(&file_path).expect("Failed to read processes.json file");
    let processes: Vec<Process> =
        serde_json::from_str(&json_content).expect("Failed to deserialize processes.json");

    for process in processes {
        if let Some((program, args)) = process.command.split_first() {
            let process_name = program.split('/').next_back().unwrap();
            let timestamp = Local::now().format("%Y-%m-%d-%H-%M-%S-%f");

            let mut stdout_log_path = base_dirs.state_dir().unwrap().to_path_buf();
            stdout_log_path.push("i3-revive");
            stdout_log_path.push("stdout");
            stdout_log_path.push(format!("{}-{}.log", process_name, timestamp));
            std::fs::create_dir_all(stdout_log_path.parent().unwrap()).unwrap();
            let stdout_log = File::create(stdout_log_path).expect("Failed to create stdout log file");

            let mut stderr_log_path = base_dirs.state_dir().unwrap().to_path_buf();
            stderr_log_path.push("i3-revive");
            stderr_log_path.push("stderr");
            stderr_log_path.push(format!("{}-{}.log", process_name, timestamp));
            std::fs::create_dir_all(stderr_log_path.parent().unwrap()).unwrap();
            let stderr_log = File::create(stderr_log_path).expect("Failed to create stderr log file");

            Command::new(program)
                .args(args)
                .current_dir(&process.working_directory)
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout_log))
                .stderr(Stdio::from(stderr_log))
                .spawn()
                .expect("Failed to spawn process");
        }
    }
}

pub fn remove_processes() {
    let base_dirs = BaseDirs::new().expect("Failed to get base directories");
    let mut file_path = base_dirs.data_local_dir().to_path_buf();
    file_path.push("i3-revive");
    file_path.push("processes.json");

    if file_path.exists() {
        fs::remove_file(&file_path).expect("Failed to remove processes.json file");
    }
}
