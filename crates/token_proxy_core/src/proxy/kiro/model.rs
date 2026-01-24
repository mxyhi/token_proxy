pub(crate) fn determine_agentic_mode(model: &str) -> (bool, bool) {
    let trimmed = model.trim();
    let is_agentic = trimmed.ends_with("-agentic");
    let is_chat_only = trimmed.ends_with("-chat");
    (is_agentic, is_chat_only)
}

pub(crate) fn map_model_to_kiro(model: &str) -> String {
    let normalized = model.trim();
    match normalized {
        // Amazon Q prefix
        "amazonq-auto" => "auto",
        "amazonq-claude-opus-4-5" => "claude-opus-4.5",
        "amazonq-claude-sonnet-4-5" => "claude-sonnet-4.5",
        "amazonq-claude-sonnet-4-5-20250929" => "claude-sonnet-4.5",
        "amazonq-claude-sonnet-4" => "claude-sonnet-4",
        "amazonq-claude-sonnet-4-20250514" => "claude-sonnet-4",
        "amazonq-claude-haiku-4-5" => "claude-haiku-4.5",
        // Kiro prefix
        "kiro-claude-opus-4-5" => "claude-opus-4.5",
        "kiro-claude-sonnet-4-5" => "claude-sonnet-4.5",
        "kiro-claude-sonnet-4-5-20250929" => "claude-sonnet-4.5",
        "kiro-claude-sonnet-4" => "claude-sonnet-4",
        "kiro-claude-sonnet-4-20250514" => "claude-sonnet-4",
        "kiro-claude-haiku-4-5" => "claude-haiku-4.5",
        "kiro-auto" => "auto",
        // Native format
        "claude-opus-4-5" => "claude-opus-4.5",
        "claude-opus-4.5" => "claude-opus-4.5",
        "claude-haiku-4-5" => "claude-haiku-4.5",
        "claude-haiku-4.5" => "claude-haiku-4.5",
        "claude-sonnet-4-5" => "claude-sonnet-4.5",
        "claude-sonnet-4-5-20250929" => "claude-sonnet-4.5",
        "claude-sonnet-4.5" => "claude-sonnet-4.5",
        "claude-sonnet-4" => "claude-sonnet-4",
        "claude-sonnet-4-20250514" => "claude-sonnet-4",
        "auto" => "auto",
        // Agentic variants
        "claude-opus-4.5-agentic" => "claude-opus-4.5",
        "claude-sonnet-4.5-agentic" => "claude-sonnet-4.5",
        "claude-sonnet-4-agentic" => "claude-sonnet-4",
        "claude-haiku-4.5-agentic" => "claude-haiku-4.5",
        "kiro-claude-opus-4-5-agentic" => "claude-opus-4.5",
        "kiro-claude-sonnet-4-5-agentic" => "claude-sonnet-4.5",
        "kiro-claude-sonnet-4-agentic" => "claude-sonnet-4",
        "kiro-claude-haiku-4-5-agentic" => "claude-haiku-4.5",
        _ => {
            let lower = normalized.to_ascii_lowercase();
            if lower.contains("haiku") {
                return "claude-haiku-4.5".to_string();
            }
            if lower.contains("sonnet") {
                if lower.contains("3-7") || lower.contains("3.7") {
                    return "claude-3-7-sonnet-20250219".to_string();
                }
                if lower.contains("4-5") || lower.contains("4.5") {
                    return "claude-sonnet-4.5".to_string();
                }
                return "claude-sonnet-4".to_string();
            }
            if lower.contains("opus") {
                return "claude-opus-4.5".to_string();
            }
            return "claude-sonnet-4.5".to_string();
        }
    }
    .to_string()
}
