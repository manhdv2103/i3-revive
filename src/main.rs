use i3_revive::{
    i3_tree::{find_workspaces, get_all_windows, save_workspaces},
    i3ipc::{connect_i3, get_tree},
    process::save_processes,
};

fn main() {
    let mut stream = connect_i3().expect("Failed to connect to i3");
    let root = get_tree(&mut stream).expect("Failed to get tree");

    let workspaces = find_workspaces(root);
    let windows = get_all_windows(&workspaces);

    save_workspaces(workspaces);
    save_processes(windows);
}
