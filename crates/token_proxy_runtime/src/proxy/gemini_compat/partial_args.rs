//! Vertex partialArgs are a structured path protocol, not JSON text fragments.
//! Buffer one bounded call until complete; never emit a fabricated empty call.
use serde_json::{json, Map, Value};
use std::collections::HashSet;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_NODES: usize = 16_384;
const MAX_DEPTH: usize = 64;
const MAX_INDEX: usize = 4095;

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
enum Segment {
    Key(String),
    Index(usize),
}

pub(super) fn is_partial(call: &Map<String, Value>) -> bool {
    let partial = field(call, "partialArgs", "partial_args");
    let continuing = will_continue(call);
    if call.get("args").is_some()
        && !continuing
        && partial.is_none_or(|value| value.as_array().is_some_and(Vec::is_empty))
    {
        return false;
    }
    partial.is_some() || field(call, "willContinue", "will_continue").is_some()
}

fn field<'a>(value: &'a Map<String, Value>, camel: &str, snake: &str) -> Option<&'a Value> {
    value.get(camel).or_else(|| value.get(snake))
}

fn will_continue(value: &Map<String, Value>) -> bool {
    field(value, "willContinue", "will_continue").and_then(Value::as_bool) == Some(true)
}

pub(super) struct PartialCall {
    pub(super) name: String,
    pub(super) id: String,
    pub(super) signature: Option<String>,
    args: Value,
    written: HashSet<Vec<Segment>>,
    open_strings: HashSet<Vec<Segment>>,
    bytes: usize,
    nodes: usize,
}

impl PartialCall {
    pub(super) fn new(call: &Map<String, Value>, signature: Option<&str>) -> Result<Self, String> {
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or("Gemini partial function call is missing a name.")?;
        Ok(Self {
            name: name.into(),
            id: call.get("id").and_then(Value::as_str).unwrap_or("").into(),
            signature: signature.map(str::to_string),
            args: json!({}),
            written: HashSet::new(),
            open_strings: HashSet::new(),
            bytes: 0,
            nodes: 0,
        })
    }

    pub(super) fn push(&mut self, call: &Map<String, Value>) -> Result<bool, String> {
        for (field_name, expected) in [("name", self.name.as_str()), ("id", self.id.as_str())] {
            if call
                .get(field_name)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty() && !expected.is_empty() && value != expected)
            {
                return Err(
                    "Gemini partial function call changed identity before completion.".into(),
                );
            }
        }
        self.bytes = self.bytes.saturating_add(
            serde_json::to_vec(call)
                .map_err(|_| "Invalid Gemini partial arguments.")?
                .len(),
        );
        if self.bytes > MAX_BYTES {
            return Err("Gemini partial arguments exceed the byte limit.".into());
        }
        if let Some(parts) = field(call, "partialArgs", "partial_args") {
            for part in parts
                .as_array()
                .ok_or("Gemini partialArgs must be an array.")?
            {
                self.push_part(
                    part.as_object()
                        .ok_or("Gemini partialArgs entry must be an object.")?,
                )?;
            }
        }
        if let Some(args) = call.get("args") {
            if !args.is_object() || !self.written.is_empty() {
                return Err("Gemini partial arguments conflict with complete arguments.".into());
            }
            self.args = args.clone();
        }
        Ok(!will_continue(call))
    }

    fn push_part(&mut self, part: &Map<String, Value>) -> Result<(), String> {
        let path = field(part, "jsonPath", "json_path")
            .and_then(Value::as_str)
            .unwrap_or("$");
        let segments = parse_path(path)?;
        let string = field(part, "stringValue", "string_value").filter(|value| !value.is_null());
        let number = field(part, "numberValue", "number_value").filter(|value| !value.is_null());
        let boolean = field(part, "boolValue", "bool_value").filter(|value| !value.is_null());
        let null = field(part, "nullValue", "null_value");
        let values = usize::from(string.is_some())
            + usize::from(number.is_some())
            + usize::from(boolean.is_some());
        if values > 1 {
            return Err("Gemini partial argument has conflicting value types.".into());
        }
        if values == 0 && null.is_none() && !will_continue(part) {
            self.open_strings.remove(&segments);
            return Ok(());
        }
        if segments.is_empty() {
            return Err("Gemini partial argument requires a leaf path.".into());
        }
        let is_string = string.is_some() || (values == 0 && will_continue(part));
        let value = if is_string {
            Value::String(
                string
                    .map(|value| value.as_str().ok_or("Invalid Gemini string argument."))
                    .transpose()?
                    .unwrap_or("")
                    .into(),
            )
        } else if let Some(number) = number {
            let parsed = if number.is_number() {
                number.clone()
            } else {
                serde_json::from_str::<Value>(
                    number.as_str().ok_or("Invalid Gemini numeric argument.")?,
                )
                .map_err(|_| "Invalid Gemini numeric argument.")?
            };
            if !parsed.is_number() {
                return Err("Invalid Gemini numeric argument.".into());
            }
            parsed
        } else if let Some(boolean) = boolean {
            Value::Bool(
                boolean
                    .as_bool()
                    .ok_or("Invalid Gemini boolean argument.")?,
            )
        } else {
            Value::Null
        };
        if self.written.iter().any(|path| {
            path != &segments && (path.starts_with(&segments) || segments.starts_with(path))
        }) {
            return Err("Gemini partial argument overlaps an existing leaf.".into());
        }
        let append = self.written.contains(&segments);
        if append && (!is_string || !self.open_strings.contains(&segments)) {
            return Err("Gemini partial argument overwrote a completed value.".into());
        }
        let target = target(&mut self.args, &segments, &mut self.nodes)?;
        if append {
            target
                .as_str()
                .ok_or("Gemini partial string type changed.")?;
            let mut text = target.as_str().unwrap_or("").to_string();
            text.push_str(value.as_str().unwrap_or(""));
            *target = Value::String(text);
        } else {
            *target = value;
        }
        self.written.insert(segments.clone());
        if is_string && will_continue(part) {
            self.open_strings.insert(segments);
        } else {
            self.open_strings.remove(&segments);
        }
        Ok(())
    }

    pub(super) fn into_call(self) -> Map<String, Value> {
        let mut call = Map::new();
        call.insert("name".into(), json!(self.name));
        if !self.id.is_empty() {
            call.insert("id".into(), json!(self.id));
        }
        call.insert("args".into(), self.args);
        call
    }
}

fn target<'a>(
    value: &'a mut Value,
    path: &[Segment],
    nodes: &mut usize,
) -> Result<&'a mut Value, String> {
    if path.is_empty() {
        return Ok(value);
    }
    *nodes += 1;
    if *nodes > MAX_NODES {
        return Err("Gemini partial arguments exceed the node limit.".into());
    }
    let child = match &path[0] {
        Segment::Key(key) => {
            if value.is_null() {
                *value = json!({});
            }
            value
                .as_object_mut()
                .ok_or("Gemini partial argument object type conflict.")?
                .entry(key.clone())
                .or_insert(Value::Null)
        }
        Segment::Index(index) => {
            if value.is_null() {
                *value = json!([]);
            }
            let array = value
                .as_array_mut()
                .ok_or("Gemini partial argument array type conflict.")?;
            if array.len() <= *index {
                *nodes += index + 1 - array.len();
                if *nodes > MAX_NODES {
                    return Err("Gemini partial arguments exceed the node limit.".into());
                }
                array.resize(index + 1, Value::Null);
            }
            &mut array[*index]
        }
    };
    target(child, &path[1..], nodes)
}

fn parse_path(path: &str) -> Result<Vec<Segment>, String> {
    let invalid = || "Invalid Gemini partial argument JSON path.".to_string();
    let mut rest = path.strip_prefix('$').ok_or_else(invalid)?;
    let mut segments = Vec::new();
    while !rest.is_empty() {
        if segments.len() >= MAX_DEPTH {
            return Err(invalid());
        }
        if let Some(tail) = rest.strip_prefix('.') {
            let end = tail.find(['.', '[']).unwrap_or(tail.len());
            if end == 0 || tail[..end].chars().any(char::is_control) {
                return Err(invalid());
            }
            segments.push(Segment::Key(tail[..end].to_string()));
            rest = &tail[end..];
        } else if let Some(tail) = rest.strip_prefix('[') {
            if tail.starts_with(['\'', '"']) {
                let quote = tail.as_bytes()[0];
                let mut escaped = false;
                let mut end = None;
                for (index, byte) in tail.bytes().enumerate().skip(1) {
                    if !escaped && byte == quote {
                        end = Some(index);
                        break;
                    }
                    if byte == b'\\' && !escaped {
                        escaped = true;
                    } else {
                        escaped = false;
                    }
                }
                let end = end.ok_or_else(invalid)?;
                let quoted = if quote == b'"' {
                    tail[..=end].to_string()
                } else {
                    // JSONPath permits single quotes; normalize only delimiters.
                    let mut output = String::from("\"");
                    let mut chars = tail[1..end].chars().peekable();
                    while let Some(ch) = chars.next() {
                        if ch == '\\' && chars.peek() == Some(&'\'') {
                            chars.next();
                            output.push('\'');
                        } else if ch == '"' {
                            output.push_str("\\\"");
                        } else {
                            output.push(ch);
                            if ch == '\\' {
                                output.push(chars.next().ok_or_else(invalid)?);
                            }
                        }
                    }
                    output.push('"');
                    output
                };
                let key = serde_json::from_str::<String>(&quoted).map_err(|_| invalid())?;
                segments.push(Segment::Key(key));
                rest = tail[end + 1..].strip_prefix(']').ok_or_else(invalid)?;
            } else {
                let (index, tail) = tail.split_once(']').ok_or_else(invalid)?;
                if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(invalid());
                }
                let index = index.parse::<usize>().map_err(|_| invalid())?;
                if index > MAX_INDEX {
                    return Err(invalid());
                }
                segments.push(Segment::Index(index));
                rest = tail;
            }
        } else {
            return Err(invalid());
        }
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_nested_partial_arguments_and_rejects_overwrites() {
        let first = json!({"name":"f","partialArgs":[{"jsonPath":"$['query']","stringValue":"你","willContinue":true},{"jsonPath":"$.items[1].n","numberValue":0}],"willContinue":true});
        let mut call = PartialCall::new(first.as_object().unwrap(), None).unwrap();
        assert!(!call.push(first.as_object().unwrap()).unwrap());
        assert!(call.push(json!({"partial_args":[{"json_path":"$.query","string_value":"好"},{"jsonPath":"$.items[0].ok","boolValue":false},{"jsonPath":"$.missing","nullValue":"NULL_VALUE"}],"will_continue":false}).as_object().unwrap()).unwrap());
        assert!(call
            .push(
                json!({"partialArgs":[{"jsonPath":"$.query","stringValue":"overwrite"}]})
                    .as_object()
                    .unwrap()
            )
            .is_err());
        assert_eq!(
            call.into_call()["args"],
            json!({"query":"你好","items":[{"ok":false},{"n":0}],"missing":null})
        );
    }

    #[test]
    fn bounds_partial_argument_paths_and_sizes() {
        for path in ["$.x[4096]", "$[-1]", "$..a", "$.x[not-an-index]"] {
            assert!(parse_path(path).is_err());
        }
        let first = json!({"name":"f","partialArgs":[{"jsonPath":"$.x","stringValue":"x".repeat(MAX_BYTES)}]});
        assert!(PartialCall::new(first.as_object().unwrap(), None)
            .unwrap()
            .push(first.as_object().unwrap())
            .is_err());
    }
}
