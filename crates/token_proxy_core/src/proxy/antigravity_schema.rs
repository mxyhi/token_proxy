use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum PathSegment {
    Key(String),
    Index(usize),
}

type Path = Vec<PathSegment>;

const UNSUPPORTED_CONSTRAINTS: [&str; 10] = [
    "minLength",
    "maxLength",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "pattern",
    "minItems",
    "maxItems",
    "format",
    "default",
    "examples",
];

pub(crate) fn clean_json_schema_for_antigravity(schema: &mut Value) {
    convert_refs_to_hints(schema);
    convert_const_to_enum(schema);
    convert_enum_values_to_strings(schema);
    add_enum_hints(schema);
    add_additional_properties_hints(schema);
    move_constraints_to_description(schema);

    merge_all_of(schema);
    flatten_any_of_one_of(schema);
    flatten_type_arrays(schema);

    remove_unsupported_keywords(schema);
    cleanup_required_fields(schema);
    add_empty_schema_placeholder(schema);
}

fn convert_refs_to_hints(schema: &mut Value) {
    let mut paths = collect_paths(schema, "$ref");
    sort_by_depth(&mut paths);
    for path in paths {
        let Some(value) = get_value(schema, &path) else {
            continue;
        };
        let ref_val = value.as_str().unwrap_or_default();
        let def_name = ref_val.rsplit('/').next().unwrap_or(ref_val);
        let mut hint = format!("See: {def_name}");
        let parent_path = match parent_path(&path) {
            Some(parent) => parent,
            None => continue,
        };
        if let Some(existing) = get_description(schema, &parent_path) {
            if !existing.is_empty() {
                hint = format!("{existing} ({hint})");
            }
        }
        let mut replacement = Map::new();
        replacement.insert("type".to_string(), Value::String("object".to_string()));
        replacement.insert("description".to_string(), Value::String(hint));
        let _ = set_value_at_path(schema, &parent_path, Value::Object(replacement));
    }
}

fn convert_const_to_enum(schema: &mut Value) {
    let paths = collect_paths(schema, "const");
    for path in paths {
        let value = get_value(schema, &path).cloned();
        let Some(value) = value else {
            continue;
        };
        let parent_path = match parent_path(&path) {
            Some(parent) => parent,
            None => continue,
        };
        let Some(parent) = get_object_mut(schema, &parent_path) else {
            continue;
        };
        if !parent.contains_key("enum") {
            parent.insert("enum".to_string(), Value::Array(vec![value.clone()]));
        }
    }
}

fn convert_enum_values_to_strings(schema: &mut Value) {
    let paths = collect_paths(schema, "enum");
    for path in paths {
        let Some(Value::Array(values)) = get_value_mut(schema, &path) else {
            continue;
        };
        let mut needs_conversion = false;
        for item in values.iter() {
            if !item.is_string() {
                needs_conversion = true;
                break;
            }
        }
        if !needs_conversion {
            continue;
        }
        let next = values
            .iter()
            .map(value_to_string)
            .map(Value::String)
            .collect::<Vec<_>>();
        *values = next;
    }
}

fn add_enum_hints(schema: &mut Value) {
    let paths = collect_paths(schema, "enum");
    for path in paths {
        let Some(Value::Array(values)) = get_value(schema, &path) else {
            continue;
        };
        if values.len() <= 1 || values.len() > 10 {
            continue;
        }
        let hint = values
            .iter()
            .map(value_to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let parent_path = match parent_path(&path) {
            Some(parent) => parent,
            None => continue,
        };
        append_hint(schema, &parent_path, &format!("Allowed: {hint}"));
    }
}

fn add_additional_properties_hints(schema: &mut Value) {
    let paths = collect_paths(schema, "additionalProperties");
    for path in paths {
        let Some(Value::Bool(false)) = get_value(schema, &path) else {
            continue;
        };
        let parent_path = match parent_path(&path) {
            Some(parent) => parent,
            None => continue,
        };
        append_hint(schema, &parent_path, "No extra properties allowed");
    }
}

fn move_constraints_to_description(schema: &mut Value) {
    for key in UNSUPPORTED_CONSTRAINTS {
        let paths = collect_paths(schema, key);
        for path in paths {
            let Some(value) = get_value(schema, &path) else {
                continue;
            };
            if value.is_object() || value.is_array() {
                continue;
            }
            let parent_path = match parent_path(&path) {
                Some(parent) => parent,
                None => continue,
            };
            if is_property_definition(&parent_path) {
                continue;
            }
            append_hint(
                schema,
                &parent_path,
                &format!("{key}: {}", value_to_string(value)),
            );
        }
    }
}

fn merge_all_of(schema: &mut Value) {
    let mut paths = collect_paths(schema, "allOf");
    sort_by_depth(&mut paths);
    for path in paths {
        let items = match get_value(schema, &path).and_then(Value::as_array) {
            Some(items) => items.clone(),
            None => continue,
        };
        let parent_path = match parent_path(&path) {
            Some(parent) => parent,
            None => continue,
        };
        {
            let Some(parent) = get_object_mut(schema, &parent_path) else {
                continue;
            };
            for item in items {
                if let Some(props) = item.get("properties").and_then(Value::as_object) {
                    let target = parent
                        .entry("properties".to_string())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(target) = target.as_object_mut() {
                        for (key, value) in props {
                            target.insert(key.clone(), value.clone());
                        }
                    }
                }
                if let Some(req) = item.get("required").and_then(Value::as_array) {
                    let required = parent
                        .entry("required".to_string())
                        .or_insert_with(|| Value::Array(Vec::new()));
                    let Some(required) = required.as_array_mut() else {
                        continue;
                    };
                    for value in req {
                        if let Some(text) = value.as_str() {
                            if !required.iter().any(|item| item.as_str() == Some(text)) {
                                required.push(Value::String(text.to_string()));
                            }
                        }
                    }
                }
            }
        }
        let _ = delete_at_path(schema, &path);
    }
}

fn flatten_any_of_one_of(schema: &mut Value) {
    for key in ["anyOf", "oneOf"] {
        let mut paths = collect_paths(schema, key);
        sort_by_depth(&mut paths);
        for path in paths {
            let Some(Value::Array(items)) = get_value(schema, &path) else {
                continue;
            };
            if items.is_empty() {
                continue;
            }
            let parent_path = match parent_path(&path) {
                Some(parent) => parent,
                None => continue,
            };
            let parent_desc = get_description(schema, &parent_path).unwrap_or_default();
            let (best_idx, all_types) = select_best(items);
            let mut selected = items[best_idx].clone();
            if !parent_desc.is_empty() {
                merge_description(&mut selected, &parent_desc);
            }
            if all_types.len() > 1 {
                append_hint_raw(
                    &mut selected,
                    &format!("Accepts: {}", all_types.join(" | ")),
                );
            }
            let _ = set_value_at_path(schema, &parent_path, selected);
        }
    }
}

fn select_best(items: &[Value]) -> (usize, Vec<String>) {
    let mut best_idx = 0;
    let mut best_score = -1;
    let mut types = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        let mut score = 0;
        let mut typ = item.get("type").and_then(Value::as_str).unwrap_or("").to_string();
        if typ == "object" || item.get("properties").is_some() {
            score = 3;
            if typ.is_empty() {
                typ = "object".to_string();
            }
        } else if typ == "array" || item.get("items").is_some() {
            score = 2;
            if typ.is_empty() {
                typ = "array".to_string();
            }
        } else if !typ.is_empty() && typ != "null" {
            score = 1;
        } else if typ.is_empty() {
            typ = "null".to_string();
        }
        if !typ.is_empty() {
            types.push(typ.clone());
        }
        if score > best_score {
            best_score = score;
            best_idx = idx;
        }
    }
    (best_idx, types)
}

fn flatten_type_arrays(schema: &mut Value) {
    let mut paths = collect_paths(schema, "type");
    sort_by_depth(&mut paths);
    let mut nullable_fields: HashMap<Path, Vec<String>> = HashMap::new();

    for path in paths {
        let Some(info) = parse_type_array(schema, &path) else {
            continue;
        };
        apply_type_array(schema, &path, &info, &mut nullable_fields);
    }

    apply_nullable_fields(schema, nullable_fields);
}

struct TypeArrayInfo {
    first: String,
    non_null: Vec<String>,
    has_null: bool,
}

fn parse_type_array(schema: &Value, path: &Path) -> Option<TypeArrayInfo> {
    let items = get_value(schema, path)?.as_array()?.clone();
    if items.is_empty() {
        return None;
    }
    let mut has_null = false;
    let mut non_null = Vec::new();
    for item in items.iter() {
        let text = value_to_string(item);
        if text == "null" {
            has_null = true;
        } else if !text.is_empty() {
            non_null.push(text);
        }
    }
    let first = non_null
        .first()
        .cloned()
        .unwrap_or_else(|| "string".to_string());
    Some(TypeArrayInfo {
        first,
        non_null,
        has_null,
    })
}

fn apply_type_array(
    schema: &mut Value,
    path: &Path,
    info: &TypeArrayInfo,
    nullable_fields: &mut HashMap<Path, Vec<String>>,
) {
    if let Some(value) = get_value_mut(schema, path) {
        *value = Value::String(info.first.clone());
    }
    let Some(parent_path) = parent_path(path) else {
        return;
    };
    if info.non_null.len() > 1 {
        append_hint(
            schema,
            &parent_path,
            &format!("Accepts: {}", info.non_null.join(" | ")),
        );
    }
    if !info.has_null {
        return;
    }
    let Some((object_path, field_name)) = property_field_from_type_path(path) else {
        return;
    };
    let mut prop_path = object_path.clone();
    prop_path.push(PathSegment::Key("properties".to_string()));
    prop_path.push(PathSegment::Key(field_name.clone()));
    append_hint(schema, &prop_path, "(nullable)");
    nullable_fields
        .entry(object_path)
        .or_default()
        .push(field_name);
}

fn apply_nullable_fields(schema: &mut Value, nullable_fields: HashMap<Path, Vec<String>>) {
    for (object_path, fields) in nullable_fields {
        let mut req_path = object_path.clone();
        req_path.push(PathSegment::Key("required".to_string()));
        let Some(Value::Array(required)) = get_value_mut(schema, &req_path) else {
            continue;
        };
        let filtered = required
            .iter()
            .filter_map(|item| item.as_str())
            .filter(|name| !fields.iter().any(|field| field == name))
            .map(|value| Value::String(value.to_string()))
            .collect::<Vec<_>>();
        if filtered.is_empty() {
            let _ = delete_at_path(schema, &req_path);
        } else {
            *required = filtered;
        }
    }
}

fn remove_unsupported_keywords(schema: &mut Value) {
    let mut keywords = Vec::from(UNSUPPORTED_CONSTRAINTS);
    keywords.extend([
        "$schema",
        "$defs",
        "definitions",
        "const",
        "$ref",
        "additionalProperties",
        "propertyNames",
    ]);
    for key in keywords {
        let mut paths = collect_paths(schema, key);
        sort_by_depth(&mut paths);
        for path in paths {
            let parent_path = match parent_path(&path) {
                Some(parent) => parent,
                None => continue,
            };
            if is_property_definition(&parent_path) {
                continue;
            }
            let _ = delete_at_path(schema, &path);
        }
    }
}

fn cleanup_required_fields(schema: &mut Value) {
    let mut paths = collect_paths(schema, "required");
    sort_by_depth(&mut paths);
    for path in paths {
        let parent_path = match parent_path(&path) {
            Some(parent) => parent,
            None => continue,
        };
        let props_path = {
            let mut next = parent_path.clone();
            next.push(PathSegment::Key("properties".to_string()));
            next
        };
        let Some(Value::Array(required)) = get_value(schema, &path) else {
            continue;
        };
        let Some(Value::Object(props)) = get_value(schema, &props_path) else {
            continue;
        };
        let valid = required
            .iter()
            .filter_map(|item| item.as_str())
            .filter(|key| props.contains_key(*key))
            .map(|value| Value::String(value.to_string()))
            .collect::<Vec<_>>();
        if valid.len() == required.len() {
            continue;
        }
        if valid.is_empty() {
            let _ = delete_at_path(schema, &path);
        } else {
            let _ = set_value_at_path(schema, &path, Value::Array(valid));
        }
    }
}

fn add_empty_schema_placeholder(schema: &mut Value) {
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

fn collect_paths(schema: &Value, field: &str) -> Vec<Path> {
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

fn sort_by_depth(paths: &mut Vec<Path>) {
    paths.sort_by(|a, b| b.len().cmp(&a.len()));
}

fn parent_path(path: &Path) -> Option<Path> {
    if path.is_empty() {
        return None;
    }
    let mut parent = path.clone();
    parent.pop();
    Some(parent)
}

fn get_value<'a>(root: &'a Value, path: &[PathSegment]) -> Option<&'a Value> {
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

fn get_value_mut<'a>(root: &'a mut Value, path: &[PathSegment]) -> Option<&'a mut Value> {
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

fn set_value_at_path(root: &mut Value, path: &[PathSegment], value: Value) -> bool {
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

fn delete_at_path(root: &mut Value, path: &[PathSegment]) -> bool {
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

fn get_object_mut<'a>(root: &'a mut Value, path: &[PathSegment]) -> Option<&'a mut Map<String, Value>> {
    get_value_mut(root, path)?.as_object_mut()
}

fn append_hint(root: &mut Value, path: &[PathSegment], hint: &str) {
    let Some(obj) = get_object_mut(root, path) else {
        return;
    };
    let existing = obj
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let next = if existing.is_empty() {
        hint.to_string()
    } else {
        format!("{existing} ({hint})")
    };
    obj.insert("description".to_string(), Value::String(next));
}

fn append_hint_raw(schema: &mut Value, hint: &str) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    let existing = obj
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let next = if existing.is_empty() {
        hint.to_string()
    } else {
        format!("{existing} ({hint})")
    };
    obj.insert("description".to_string(), Value::String(next));
}

fn merge_description(schema: &mut Value, parent_desc: &str) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    let child_desc = obj
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    if child_desc.is_empty() {
        obj.insert("description".to_string(), Value::String(parent_desc.to_string()));
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

fn is_property_definition(path: &[PathSegment]) -> bool {
    match path.last() {
        Some(PathSegment::Key(key)) if key == "properties" => true,
        _ => path.len() == 1 && matches!(path[0], PathSegment::Key(ref key) if key == "properties"),
    }
}

fn get_description(root: &Value, path: &[PathSegment]) -> Option<String> {
    let obj = get_value(root, path)?.as_object()?;
    let desc = obj.get("description")?.as_str()?.trim().to_string();
    Some(desc)
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn property_field_from_type_path(path: &[PathSegment]) -> Option<(Path, String)> {
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
