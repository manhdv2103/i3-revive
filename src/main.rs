use chrono::Local;
use directories::BaseDirs;
use i3_revive::{
    config::load_config,
    i3_tree::{find_workspaces, get_all_windows, remove_workspaces, save_workspaces},
    i3ipc::{connect_i3, get_tree},
    process::{remove_processes, save_processes},
};
use std::{env, fs, io};

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() != 2 {
        eprintln!("Usage: {} <save|restore|rm>", args[0]);
        std::process::exit(1);
    }

    let command = &args[1];

    load_config();
    match command.as_str() {
        "save" => {
            let mut stream = connect_i3().expect("Failed to connect to i3");
            let root = get_tree(&mut stream).expect("Failed to get tree");

            let workspaces = find_workspaces(root);
            let windows = get_all_windows(&workspaces);

            // Backup and clear existing data before saving new ones
            if let Err(e) = backup_and_clear_data() {
                eprintln!("Failed to create backup: {}", e);
                std::process::exit(1);
            }

            save_workspaces(workspaces);
            save_processes(windows);
        }
        "restore" => {
            eprintln!("TODO: restore command not implemented yet");
            std::process::exit(1);
        }
        "rm" => {
            if let Err(e) = backup_and_clear_data() {
                eprintln!("Failed to create backup: {}", e);
                std::process::exit(1);
            }
            println!("Successfully removed saved layouts and processes");
        }
        _ => {
            eprintln!("Usage: {} <save|restore|rm>", args[0]);
            std::process::exit(1);
        }
    }
}

fn backup_and_clear_data() -> io::Result<()> {
    let base_dirs = BaseDirs::new().expect("Failed to get base directories");
    let mut source_dir = base_dirs.data_local_dir().to_path_buf();
    source_dir.push("i3-revive");

    let timestamp = Local::now().format("%Y_%m_%d_%H_%M_%S_%3f").to_string();
    let mut backups_dir = base_dirs.data_local_dir().to_path_buf();
    backups_dir.push("i3-revive");
    backups_dir.push("backups");
    let backup_dir = backups_dir.join(timestamp);

    fs::create_dir_all(&backup_dir)?;

    // Copy layouts directory
    let mut layouts_source = source_dir.clone();
    layouts_source.push("layouts");
    if layouts_source.exists() {
        let layouts_backup = backup_dir.join("layouts");
        fs::create_dir_all(&layouts_backup)?;

        for entry in fs::read_dir(layouts_source)? {
            let entry = entry?;
            let dest_path = layouts_backup.join(entry.file_name());
            fs::copy(entry.path(), dest_path)?;
        }
    }

    // Copy processes.json file
    let mut processes_source = source_dir.clone();
    processes_source.push("processes.json");
    if processes_source.exists() {
        let processes_backup = backup_dir.join("processes.json");
        fs::copy(processes_source, processes_backup)?;
    }

    // Clean up old backups - keep only the 100 most recent
    let mut entries = Vec::new();

    for entry in fs::read_dir(&backups_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            entries.push(entry.path());
        }
    }

    // Sort by modification time (newest first)
    entries.sort_by(|a, b| {
        let a_time = fs::metadata(a).and_then(|m| m.modified()).unwrap();
        let b_time = fs::metadata(b).and_then(|m| m.modified()).unwrap();
        b_time.cmp(&a_time)
    });

    // Remove old backups beyond the 50 limit
    for old_backup in entries.iter().skip(50) {
        if let Err(e) = fs::remove_dir_all(old_backup) {
            eprintln!(
                "Warning: Failed to remove old backup {:?}: {}",
                old_backup, e
            );
        }
    }

    // Remove existing data
    remove_workspaces();
    remove_processes();

    Ok(())
}
