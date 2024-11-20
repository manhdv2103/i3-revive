use i3_revive::i3_save_tree::save_tree;
use i3_revive::i3ipc::{connect_i3, get_tree};

fn main() {
    let mut stream = connect_i3().expect("Failed to connect to i3");
    let root = get_tree(&mut stream).expect("Failed to get tree");

    save_tree(root);
}
