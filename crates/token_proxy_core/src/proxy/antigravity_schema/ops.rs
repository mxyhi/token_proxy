use serde_json::{Map, Value};

use super::{Path, PathSegment};

pub(super) fn add_empty_schema_placeholder(schema: &mut Value) {
    let mut paths = collect_paths(schema, "type");
    sort_by_depth(&mut paths);
    for path in paths {
        let Some(Value::String(value)) = get_value(schema, &path) else {
            continue;
        };
        if value != "object" {
            continue;
        }
        let parent_path = match parent_path(&path) {
            Some(parent) => parent,
            None => continue,
        };
        apply_schema_placeholder(schema, &parent_path);
    }
}

fn apply_schema_placeholder(schema: &mut Value, parent_path: &Path) {
    let Some(parent) = get_object_mut(schema, parent_path) else {
        return;
    };
    let props = parent.get("properties");
    let req = parent.get("required");
    let has_required = req
        .and_then(Value::as_array)
        .map(|items| !items.is_empty())
        .unwrap_or(false);
    let needs_placeholder = match props {
        None => true,
        Some(Value::Object(map)) => map.is_empty(),
        _ => false,
    };
    if needs_placeholder {
        add_reason_placeholder(parent);
        return;
    }
    if !has_required {
        if parent_path.is_empty() {
            return;
        }
        add_required_placeholder(parent);
    }
}

fn add_reason_placeholder(parent: &mut Map<String, Value>) {
    let props = parent
        .entry("properties".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(props) = props.as_object_mut() else {
        return;
    };
    let reason = props
        .entry("reason".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(reason) = reason.as_object_mut() {
        reason.insert("type".to_string(), Value::String("string".to_string()));
        reason.insert(
            "description".to_string(),
            Value::String("Brief explanation of why you are calling this tool".to_string()),
        );
    }
    parent.insert(
        "required".to_string(),
        Value::Array(vec![Value::String("reason".to_string())]),
    );
}

fn add_required_placeholder(parent: &mut Map<String, Value>) {
    let props = parent
        .entry("properties".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(props) = props.as_object_mut() else {
        return;
    };
    if !props.contains_key("_") {
        let mut placeholder = Map::new();
        placeholder.insert("type".to_string(), Value::String("boolean".to_string()));
        props.insert("_".to_string(), Value::Object(placeholder));
    }
    parent.insert(
        "required".to_string(),
        Value::Array(vec![Value::String("_".to_string())]),
    );
}

pub(super) fn collect_paths(schema: &Value, field: &str) -> Vec<Path> {
    let mut paths = Vec::new();
    let mut current = Vec::new();
    walk(schema, field, &mut current, &mut paths);
    paths
}

fn walk(value: &Value, field: &str, path: &mut Path, out: &mut Vec<Path>) {
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                path.push(PathSegment::Key(key.clone()));
                if key == field {
                    out.push(path.clone());
                }
                walk(val, field, path, out);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (idx, item) in items.iter().enumerate() {
                path.push(PathSegment::Index(idx));
                walk(item, field, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

pub(super) fn sort_by_depth(paths: &mut Vec<Path>) {
    paths.sort_by(|a, b| b.len().cmp(&a.len()));
}

pub(super) fn parent_path(path: &Path) -> Option<Path> {
    if path.is_empty() {
        return None;
    }
    let mut parent = path.clone();
    parent.pop();
    Some(parent)
}

pub(super) fn get_value<'a>(root: &'a Value, path: &[PathSegment]) -> Option<&'a Value> {
    let mut current = root;
    for segment in path {
        match segment {
            PathSegment::Key(key) => {
                current = current.get(key)?;
            }
            PathSegment::Index(index) => {
                current = current.get(*index)?;
            }
        }
    }
    Some(current)
}

pub(super) fn get_value_mut<'a>(
    root: &'a mut Value,
    path: &[PathSegment],
) -> Option<&'a mut Value> {
    let mut current = root;
    for segment in path {
        match segment {
            PathSegment::Key(key) => {
                current = current.get_mut(key)?;
            }
            PathSegment::Index(index) => {
                current = current.get_mut(*index)?;
            }
        }
    }
    Some(current)
}

pub(super) fn set_value_at_path(root: &mut Value, path: &[PathSegment], value: Value) -> bool {
    if path.is_empty() {
        *root = value;
        return true;
    }
    let (parent, last) = match split_parent(path) {
        Some(split) => split,
        None => return false,
    };
    let Some(parent) = get_value_mut(root, parent) else {
        return false;
    };
    match last {
        PathSegment::Key(key) => {
            let Some(obj) = parent.as_object_mut() else {
                return false;
            };
            obj.insert(key.clone(), value);
            true
        }
        PathSegment::Index(index) => {
            let Some(arr) = parent.as_array_mut() else {
                return false;
            };
            if *index >= arr.len() {
                return false;
            }
            arr[*index] = value;
            true
        }
    }
}

pub(super) fn delete_at_path(root: &mut Value, path: &[PathSegment]) -> bool {
    let (parent, last) = match split_parent(path) {
        Some(split) => split,
        None => return false,
    };
    let Some(parent) = get_value_mut(root, parent) else {
        return false;
    };
    match last {
        PathSegment::Key(key) => {
            let Some(obj) = parent.as_object_mut() else {
                return false;
            };
            obj.remove(key).is_some()
        }
        PathSegment::Index(index) => {
            let Some(arr) = parent.as_array_mut() else {
                return false;
            };
            if *index >= arr.len() {
                return false;
            }
            arr.remove(*index);
            true
        }
    }
}

fn split_parent(path: &[PathSegment]) -> Option<(&[PathSegment], &PathSegment)> {
    let len = path.len();
    if len == 0 {
        return None;
    }
    Some((&path[..len - 1], &path[len - 1]))
}

pub(super) fn get_object_mut<'a>(
    root: &'a mut Value,
    path: &[PathSegment],
) -> Option<&'a mut Map<String, Value>> {
    get_value_mut(root, path)?.as_object_mut()
}

pub(super) fn append_hint(root: &mut Value, path: &[PathSegment], hint: &str) {
    let Some(obj) = get_object_mut(root, path) else {
        return;
    };
    let existing = obj.get("description").and_then(Value::as_str).unwrap_or("");
    let next = if existing.is_empty() {
        hint.to_string()
    } else {
        format!("{existing} ({hint})")
    };
    obj.insert("description".to_string(), Value::String(next));
}

pub(super) fn append_hint_raw(schema: &mut Value, hint: &str) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    let existing = obj.get("description").and_then(Value::as_str).unwrap_or("");
    let next = if existing.is_empty() {
        hint.to_string()
    } else {
        format!("{existing} ({hint})")
    };
    obj.insert("description".to_string(), Value::String(next));
}

pub(super) fn merge_description(schema: &mut Value, parent_desc: &str) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    let child_desc = obj.get("description").and_then(Value::as_str).unwrap_or("");
    if child_desc.is_empty() {
        obj.insert(
            "description".to_string(),
            Value::String(parent_desc.to_string()),
        );
        return;
    }
    if child_desc == parent_desc {
        return;
    }
    obj.insert(
        "description".to_string(),
        Value::String(format!("{parent_desc} ({child_desc})")),
    );
}

pub(super) fn is_property_definition(path: &[PathSegment]) -> bool {
    match path.last() {
        Some(PathSegment::Key(key)) if key == "properties" => true,
        _ => path.len() == 1 && matches!(path[0], PathSegment::Key(ref key) if key == "properties"),
    }
}

pub(super) fn get_description(root: &Value, path: &[PathSegment]) -> Option<String> {
    let obj = get_value(root, path)?.as_object()?;
    let desc = obj.get("description")?.as_str()?.trim().to_string();
    Some(desc)
}

pub(super) fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

pub(super) fn property_field_from_type_path(path: &[PathSegment]) -> Option<(Path, String)> {
    if path.len() < 3 {
        return None;
    }
    let len = path.len();
    if !matches!(path.get(len - 3), Some(PathSegment::Key(key)) if key == "properties") {
        return None;
    }
    let field = match path.get(len - 2) {
        Some(PathSegment::Key(key)) => key.clone(),
        _ => return None,
    };
    Some((path[..len - 3].to_vec(), field))
}
