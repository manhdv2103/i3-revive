use std::{
    fs,
    io::{BufWriter, Write},
};

use directories::BaseDirs;
use regex::escape;
use serde_json::{Map, Value};

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

// https://github.com/i3/i3/blob/2746e0319b03a8a5a02b57a69b1fb47e0a9c22f1/i3-save-tree#L105
fn convert_to_layout(tree: &mut Value) {
    let tree_obj = tree.as_object_mut().unwrap();
    let is_tree_leaf_node = is_leaf_node(tree_obj);

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
    // Turn “window_properties” into “swallows” expressions, but only for leaf
    // nodes. It only makes sense for leaf nodes to swallow anything.
    if is_tree_leaf_node {
        if let Some(window_properties) = tree_obj.get("window_properties") {
            let props_obj = window_properties.as_object().unwrap();
            let mut swallows = Map::new();

            if let Some(class) = props_obj.get("class") {
                swallows.insert(
                    "class".to_string(),
                    Value::String(format!("^{}$", escape(class.as_str().unwrap()).as_str())),
                );
            }

            if let Some(class) = props_obj.get("instance") {
                swallows.insert(
                    "instance".to_string(),
                    Value::String(format!("^{}$", escape(class.as_str().unwrap()).as_str())),
                );
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

pub fn get_all_windows(trees: &Vec<Value>) -> Vec<u32> {
    let mut res: Vec<u32> = vec![];

    for tree in trees {
        if let serde_json::Value::Number(i) = tree.get("window").unwrap() {
            res.push(i.as_u64().unwrap() as u32)
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
        file_path.push(format!("ws_{}", ws_name));
        let f = fs::File::create(file_path).expect("Failed to create file");
        let mut f = BufWriter::new(f);

        run_for_all_nodes(ws, |v| {
            for child in v.as_array().unwrap().iter() {
                write!(
                    f,
                    "{}

",
                    serde_json::to_string_pretty(child).expect("Failed to serialize")
                )
                .expect("Failed to write to file");
            }
        });
    }
}