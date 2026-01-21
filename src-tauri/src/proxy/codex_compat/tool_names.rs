use std::collections::{HashMap, HashSet};

const TOOL_NAME_LIMIT: usize = 64;
const MCP_PREFIX: &str = "mcp__";

#[derive(Clone, Debug, Default)]
pub(crate) struct ToolNameMap {
    pub(crate) short_by_original: HashMap<String, String>,
    pub(crate) original_by_short: HashMap<String, String>,
}

impl ToolNameMap {
    pub(crate) fn from_names(names: &[String]) -> Self {
        let mut used = HashSet::new();
        let mut short_by_original = HashMap::new();
        let mut original_by_short = HashMap::new();

        for name in names {
            let candidate = base_candidate(name);
            let short = make_unique(&candidate, &mut used);
            short_by_original.insert(name.clone(), short.clone());
            original_by_short.insert(short, name.clone());
        }

        Self {
            short_by_original,
            original_by_short,
        }
    }

    pub(crate) fn shorten(&self, name: &str) -> String {
        self.short_by_original
            .get(name)
            .cloned()
            .unwrap_or_else(|| shorten_name_if_needed(name))
    }
}

pub(crate) fn shorten_name_if_needed(name: &str) -> String {
    if name.len() <= TOOL_NAME_LIMIT {
        return name.to_string();
    }
    base_candidate(name)
}

fn base_candidate(name: &str) -> String {
    if name.len() <= TOOL_NAME_LIMIT {
        return name.to_string();
    }
    if let Some(candidate) = shorten_mcp_name(name) {
        return candidate;
    }
    truncate_name(name, TOOL_NAME_LIMIT)
}

fn shorten_mcp_name(name: &str) -> Option<String> {
    if !name.starts_with(MCP_PREFIX) {
        return None;
    }
    let idx = name.rfind("__")?;
    if idx <= MCP_PREFIX.len() {
        return None;
    }
    let mut candidate = format!("{MCP_PREFIX}{}", &name[idx + 2..]);
    if candidate.len() > TOOL_NAME_LIMIT {
        candidate.truncate(TOOL_NAME_LIMIT);
    }
    Some(candidate)
}

fn truncate_name(name: &str, limit: usize) -> String {
    let mut out = name.to_string();
    if out.len() > limit {
        out.truncate(limit);
    }
    out
}

fn make_unique(candidate: &str, used: &mut HashSet<String>) -> String {
    if used.insert(candidate.to_string()) {
        return candidate.to_string();
    }
    for index in 1.. {
        let suffix = format!("_{index}");
        let allowed = TOOL_NAME_LIMIT.saturating_sub(suffix.len());
        let mut base = candidate.to_string();
        if base.len() > allowed {
            base.truncate(allowed);
        }
        let next = format!("{base}{suffix}");
        if used.insert(next.clone()) {
            return next;
        }
    }
    candidate.to_string()
}
