pub(crate) fn redact_query_param_value(message: &str, name: &str) -> String {
    let needle = format!("{name}=");
    let mut output = String::with_capacity(message.len());
    let mut rest = message;

    while let Some(pos) = rest.find(&needle) {
        let (before, after) = rest.split_at(pos);
        output.push_str(before);
        output.push_str(&needle);
        output.push_str("***");

        let after = &after[needle.len()..];
        let mut end = after.len();
        for (idx, ch) in after.char_indices() {
            if matches!(ch, '&' | ')' | ' ' | '\n' | '\r' | '\t' | '"' | '\'') {
                end = idx;
                break;
            }
        }
        rest = &after[end..];
    }

    output.push_str(rest);
    output
}
