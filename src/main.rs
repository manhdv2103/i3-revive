use i3_revive::{
    config::load_config,
    i3_tree::{find_workspaces, get_all_windows, save_workspaces},
    i3ipc::{connect_i3, get_tree},
    process::save_processes,
};
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() != 2 {
        eprintln!("Usage: {} <save|restore|rm>", args[0]);
        std::process::exit(1);
    }
    
    let command = &args[1];
    
    match command.as_str() {
        "save" => {
            load_config();
            let mut stream = connect_i3().expect("Failed to connect to i3");
            let root = get_tree(&mut stream).expect("Failed to get tree");

            let workspaces = find_workspaces(root);
            let windows = get_all_windows(&workspaces);

            save_workspaces(workspaces);
            save_processes(windows);
        }
        "restore" => {
            eprintln!("TODO: restore command not implemented yet");
            std::process::exit(1);
        }
        "rm" => {
            eprintln!("TODO: rm command not implemented yet");
            std::process::exit(1);
        }
        _ => {
            eprintln!("Usage: {} <save|restore|rm>", args[0]);
            std::process::exit(1);
        }
    }
}
