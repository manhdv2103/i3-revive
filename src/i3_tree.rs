use std::{
    collections::HashSet,
    error::Error,
    fs,
    io::{BufWriter, Write},
    os::unix::net::UnixStream,
};

use directories::BaseDirs;
use regex::{escape, Regex};
use serde_json::{Map, Value};
use xcb::{x, XidNew};

use crate::{
    config::CONFIG,
    i3ipc::{get_tree, get_workspaces, run_command},
};

#[derive(Debug)]
pub struct Window {
    pub id: u32,
    pub name: String,
    pub class: Option<String>,
    pub is_placeholder: bool,
}

// https://github.com/i3/i3/blob/2746e0319b03a8a5a02b57a69b1fb47e0a9c22f1/i3-save-tree#L88
const ALLOWED_KEYS: &[&str] = &[
    "type",
    "fullscreen_mode",
    "layout",
    "border",
    "current_border_width",
    "floating",
    "percent",
    "nodes",
    "floating_nodes",
    "name",
    "geometry",
    "window_properties",
    "marks",
    "rect",
];

pub fn find_workspaces(mut tree: Value) -> Vec<Value> {
    if is_normal_workspace(&tree) {
        return vec![tree];
    }

    let mut res: Vec<Value> = vec![];
    run_for_all_nodes_mut(&mut tree, |v| {
        for child in v.as_array_mut().unwrap().drain(..) {
            res.append(&mut find_workspaces(child))
        }
    });

    res
}

pub fn get_all_windows(trees: &Vec<Value>) -> Vec<Window> {
    let mut res: Vec<Window> = vec![];

    for tree in trees {
        if let (Some(id), Some(name), Some(props)) = (
            tree.get("window"),
            tree.get("name"),
            tree.get("window_properties"),
        ) {
            res.push(Window {
                id: id.as_u64().unwrap() as u32,
                name: name.as_str().unwrap().to_string(),
                class: props
                    .get("class")
                    .and_then(|class| class.as_str())
                    .map(|str| str.to_string()),
                is_placeholder: tree
                    .get("swallows")
                    .is_some_and(|swallows| !swallows.as_array().unwrap().is_empty()),
            })
        };

        run_for_all_nodes(tree, |v| {
            res.append(&mut get_all_windows(v.as_array().unwrap()));
        });
    }

    res
}

pub fn save_workspaces(mut workspaces: Vec<Value>) {
    let base_dirs = BaseDirs::new().expect("Failed to get base directories");
    let mut dir = base_dirs.data_local_dir().to_path_buf();
    dir.push("i3-revive");
    dir.push("layouts");
    fs::create_dir_all(&dir).expect("Failed to create directory");

    for ws in workspaces.iter_mut() {
        let ws_name_node = ws.get("name").unwrap().to_owned();
        let ws_name = ws_name_node.as_str().unwrap();

        convert_to_layout(ws);

        let mut file_path = dir.clone();
        file_path.push(format!("ws_{}.json", ws_name));
        let f = fs::File::create(file_path).expect("Failed to create file");
        let mut f = BufWriter::new(f);

        run_for_all_nodes(ws, |v| {
            for child in v.as_array().unwrap().iter() {
                writeln!(
                    f,
                    "{}",
                    serde_json::to_string_pretty(child).expect("Failed to serialize")
                )
                .expect("Failed to write to file");
            }
        });
    }
}

pub fn restore_workspaces(stream: &mut UnixStream) {
    let base_dirs = BaseDirs::new().expect("Failed to get base directories");
    let mut dir = base_dirs.data_local_dir().to_path_buf();
    dir.push("i3-revive");
    dir.push("layouts");

    if !dir.exists() {
        eprintln!("Layouts directory is missing");
        std::process::exit(1);
    }

    let root = get_tree(stream).expect("Failed to get tree");
    let (conn, _) = xcb::Connection::connect(None).expect("Failed to connect to X server");

    let tree_workspaces = find_workspaces(root);
    let windows = get_all_windows(&tree_workspaces);
    let workspaces = get_workspaces(stream).expect("Failed to get workspaces");
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

    for window in &windows {
        if window.is_placeholder {
            conn.send_and_check_request(&x::KillClient {
                resource: window.id,
            })
            .unwrap();
        } else {
            conn.send_and_check_request(&x::UnmapWindow {
                window: unsafe { XidNew::new(window.id) },
            })
            .unwrap();
        };
    }

    let do_things = || -> Result<(), Box<dyn Error>> {
        let ws_name_re = Regex::new(r"^ws_(.+).json$").unwrap();
        let mut first_entry = true;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_name = entry
                .file_name()
                .into_string()
                .map_err(|os_str| format!("Failed to convert file name to string: {:?}", os_str))?;
            let path = entry
                .path()
                .into_os_string()
                .into_string()
                .map_err(|os_str| format!("Failed to convert path to string: {:?}", os_str))?;

            if let Some(caps) = ws_name_re.captures(&file_name) {
                let ws_name = &caps[1];

                if !first_entry || focused_workspace_name.is_none_or(|name| name != ws_name) {
                    run_command(stream, format!("workspace {}", ws_name).as_str())?;
                }

                run_command(stream, format!("append_layout {}", path).as_str())?;

                first_entry = false;
            }
        }
        Ok(())
    };

    let has_err = if let Err(err) = do_things() {
        eprintln!("Failed to do things: {}", err);
        false
    } else {
        true
    };

    for window in &windows {
        if !window.is_placeholder {
            conn.send_and_check_request(&x::MapWindow {
                window: unsafe { XidNew::new(window.id) },
            })
            .unwrap();
        }
    }

    if has_err {
        std::process::exit(1);
    }
}

pub fn remove_workspaces() {
    let base_dirs = BaseDirs::new().expect("Failed to get base directories");
    let mut dir = base_dirs.data_local_dir().to_path_buf();
    dir.push("i3-revive");
    dir.push("layouts");

    if dir.exists() {
        fs::remove_dir_all(&dir).expect("Failed to remove layouts directory");
    }
}

// https://github.com/i3/i3/blob/2746e0319b03a8a5a02b57a69b1fb47e0a9c22f1/i3-save-tree#L105
fn convert_to_layout(tree: &mut Value) {
    let config = CONFIG.get().unwrap();
    let tree_obj = tree.as_object_mut().unwrap();
    let is_tree_leaf_node = is_leaf_node(tree_obj);
    let leaf_node_id = tree_obj.get("window").and_then(|win| win.as_u64());

    // layout is not relevant for a leaf container
    if is_tree_leaf_node {
        tree_obj.remove("layout");
    }

    // fullscreen_mode conveys no state at all, it can be 0, 1, or 2 and the
    // default is _always_ 0, so skip noop entries.
    if tree_obj.get("fullscreen_mode").unwrap().as_u64().unwrap() == 0 {
        tree_obj.remove("fullscreen_mode");
    }

    // names for non-leafs are auto-generated and useful only for i3 debugging
    if !is_tree_leaf_node {
        tree_obj.remove("name");
    }

    if is_zero_rect(tree_obj.get("geometry").unwrap().as_object().unwrap()) {
        tree_obj.remove("geometry");
    };

    // Retain the rect for floating containers to keep their positions
    if tree_obj.get("type").unwrap().as_str().unwrap() != "floating_con" {
        tree_obj.remove("rect");
    }

    if tree_obj
        .get("current_border_width")
        .unwrap()
        .as_i64()
        .unwrap()
        == -1
    {
        tree_obj.remove("current_border_width");
    }

    if tree_obj
        .get("nodes")
        .is_some_and(|v| v.as_array().unwrap().is_empty())
    {
        tree_obj.remove("nodes");
    }

    if tree_obj
        .get("floating_nodes")
        .is_some_and(|v| v.as_array().unwrap().is_empty())
    {
        tree_obj.remove("floating_nodes");
    }

    let keys = tree_obj
        .iter()
        .map(|(key, _)| key.to_owned())
        .collect::<Vec<_>>();
    for key in keys {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            tree_obj.remove(&key);
        }
    }

    // https://github.com/i3/i3/blob/2746e0319b03a8a5a02b57a69b1fb47e0a9c22f1/i3-save-tree#L167
    // Turn "window_properties" into "swallows" expressions, but only for leaf
    // nodes. It only makes sense for leaf nodes to swallow anything.
    if is_tree_leaf_node {
        if let Some(window_properties) = tree_obj.get("window_properties") {
            let props_obj = window_properties.as_object().unwrap();
            let mut swallows = Map::new();
            let mut criteria: Option<&HashSet<String>> = None;
            let mut is_terminal = false;

            if let Some(class) = props_obj.get("class") {
                let class = class.as_str().unwrap();
                criteria = config.window_swallow_criteria.get(class);

                if criteria.is_none_or(|crit| crit.contains("class")) {
                    swallows.insert(
                        "class".to_string(),
                        Value::String(format!("^{}$", escape(class).as_str())),
                    );
                }

                is_terminal = config.terminal_revive_commands.contains_key(class);
            }

            if let Some(instance) = props_obj.get("instance") {
                if criteria.is_none_or(|crit| crit.contains("instance")) {
                    swallows.insert(
                        "instance".to_string(),
                        Value::String(format!("^{}$", escape(instance.as_str().unwrap()).as_str())),
                    );
                }
            }

            if let Some(title) = props_obj.get("title") {
                let title = title.as_str().unwrap();
                if criteria.is_some_and(|crit| crit.contains("title")) {
                    swallows.insert(
                        "title".to_string(),
                        Value::String(format!("^{}$", escape(title).as_str())),
                    );
                } else if is_terminal {
                    swallows.insert(
                        "title".to_string(),
                        Value::String(format!("Terminal-Window-{}", leaf_node_id.unwrap(),)),
                    );
                }
            }

            tree_obj.insert(
                "swallows".to_string(),
                Value::Array(vec![Value::Object(swallows)]),
            );
        }
    }
    tree_obj.remove("window_properties");

    run_for_all_nodes_mut(tree, |v| {
        for child in v.as_array_mut().unwrap().iter_mut() {
            convert_to_layout(child)
        }
    });
}

fn run_for_all_nodes(tree: &Value, mut cb: impl FnMut(&Value)) {
    if let Some(v) = tree.get("nodes") {
        cb(v);
    }
    if let Some(v) = tree.get("floating_nodes") {
        cb(v);
    }
}

fn run_for_all_nodes_mut(tree: &mut Value, mut cb: impl FnMut(&mut Value)) {
    if let Some(v) = tree.get_mut("nodes") {
        cb(v);
    }
    if let Some(v) = tree.get_mut("floating_nodes") {
        cb(v);
    }
}

fn is_normal_workspace(val: &Value) -> bool {
    val.get("type").unwrap().as_str().unwrap() == "workspace"
        && val.get("name").is_some_and(|n| n != "__i3_scratch")
}

fn is_leaf_node(val: &Map<String, Value>) -> bool {
    val.get("type").unwrap().as_str().unwrap() == "con"
        && val
            .get("nodes")
            .is_none_or(|ns| ns.as_array().unwrap().is_empty())
        && val
            .get("floating_nodes")
            .is_none_or(|ns| ns.as_array().unwrap().is_empty())
}

fn is_zero_rect(val: &Map<String, Value>) -> bool {
    val.get("x").unwrap().as_u64().unwrap() == 0
        && val.get("y").unwrap().as_u64().unwrap() == 0
        && val.get("width").unwrap().as_u64().unwrap() == 0
        && val.get("height").unwrap().as_u64().unwrap() == 0
}
