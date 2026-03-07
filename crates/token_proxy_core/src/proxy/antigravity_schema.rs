use serde_json::{Map, Value};
use std::collections::HashMap;

mod ops;
use ops::*;

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
    clean_json_schema(schema, true);
}

pub(crate) fn clean_json_schema_for_gemini(schema: &mut Value) {
    clean_json_schema(schema, false);
}

fn clean_json_schema(schema: &mut Value, add_placeholder: bool) {
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
    if !add_placeholder {
        // Gemini schema cleanup: remove nullable/title and placeholder-only fields.
        remove_keywords(schema, &["nullable", "title"]);
        remove_placeholder_fields(schema);
    }
    cleanup_required_fields(schema);
    // Antigravity/Claude VALIDATED-mode: object schemas cannot be empty; inject placeholders.
    if add_placeholder {
        add_empty_schema_placeholder(schema);
    }
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

fn remove_keywords(schema: &mut Value, keywords: &[&str]) {
    for key in keywords {
        let mut paths = collect_paths(schema, key);
        sort_by_depth(&mut paths);
        for path in paths {
            let Some(parent_path) = parent_path(&path) else {
                continue;
            };
            if is_property_definition(&parent_path) {
                continue;
            }
            let _ = delete_at_path(schema, &path);
        }
    }
}

fn remove_placeholder_fields(schema: &mut Value) {
    remove_placeholder_property(schema, "_", None);
    remove_placeholder_reason(schema);
}

fn remove_placeholder_property(schema: &mut Value, key: &str, required_key: Option<&str>) {
    let mut paths = collect_paths(schema, key);
    sort_by_depth(&mut paths);
    for path in paths {
        if !ends_with_properties_key(&path, key) {
            continue;
        }
        let _ = delete_at_path(schema, &path);
        let Some(parent_path) = trim_properties_key_suffix(&path, key) else {
            continue;
        };
        remove_required_entry(schema, &parent_path, required_key.unwrap_or(key));
    }
}

fn remove_placeholder_reason(schema: &mut Value) {
    let mut paths = collect_paths(schema, "reason");
    sort_by_depth(&mut paths);
    for path in paths {
        if !ends_with_properties_key(&path, "reason") {
            continue;
        }
        let Some(parent_path) = trim_properties_key_suffix(&path, "reason") else {
            continue;
        };
        // Only remove when it's the only property and matches our placeholder description.
        let props_path = join_path(&parent_path, "properties");
        let Some(Value::Object(props)) = get_value(schema, &props_path) else {
            continue;
        };
        if props.len() != 1 {
            continue;
        }
        let desc_path = join_path(&path, "description");
        let desc = get_value(schema, &desc_path)
            .and_then(Value::as_str)
            .unwrap_or("");
        if desc != "Brief explanation of why you are calling this tool" {
            continue;
        }
        let _ = delete_at_path(schema, &path);
        remove_required_entry(schema, &parent_path, "reason");
    }
}

fn ends_with_properties_key(path: &Path, key: &str) -> bool {
    if path.len() < 2 {
        return false;
    }
    matches!(path.get(path.len() - 2), Some(PathSegment::Key(k)) if k == "properties")
        && matches!(path.last(), Some(PathSegment::Key(k)) if k == key)
}

fn trim_properties_key_suffix(path: &Path, key: &str) -> Option<Path> {
    if !ends_with_properties_key(path, key) {
        return None;
    }
    let mut parent = path.clone();
    parent.pop(); // key
    parent.pop(); // properties
    Some(parent)
}

fn join_path(parent: &Path, key: &str) -> Path {
    let mut next = parent.clone();
    next.push(PathSegment::Key(key.to_string()));
    next
}

fn remove_required_entry(schema: &mut Value, parent_path: &Path, key: &str) {
    let req_path = join_path(parent_path, "required");
    let Some(Value::Array(required)) = get_value_mut(schema, &req_path) else {
        return;
    };
    let next = required
        .iter()
        .filter_map(|item| item.as_str())
        .filter(|item| *item != key)
        .map(|item| Value::String(item.to_string()))
        .collect::<Vec<_>>();
    if next.is_empty() {
        let _ = delete_at_path(schema, &req_path);
    } else {
        *required = next;
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
        let next = values
            .iter()
            .map(value_to_string)
            .map(Value::String)
            .collect::<Vec<_>>();
        *values = next;
        let Some(parent_path) = parent_path(&path) else {
            continue;
        };
        if let Some(parent) = get_object_mut(schema, &parent_path) {
            parent.insert("type".to_string(), Value::String("string".to_string()));
        }
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
        let mut typ = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
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
