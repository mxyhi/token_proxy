use super::*;

#[test]
fn estimate_tokens_for_claude_uses_heuristic() {
    let tokens = estimate_text_tokens(Some("claude-3-opus"), "a");
    // Claude word multiplier 1.13 -> ceil => 2
    assert_eq!(tokens, 2);
}

#[test]
fn estimate_tokens_counts_japanese_as_cjk_per_char() {
    // new-api 的 isCJK 包含 0x3040..=0x30FF（假名），本项目需要对齐。
    let tokens = estimate_text_tokens(Some("claude-3-opus"), "あいうえお");
    // 5 chars * 1.21 => 6.05 -> ceil => 7
    assert_eq!(tokens, 7);
}

#[test]
fn estimate_tokens_counts_korean_as_cjk_per_char() {
    // new-api 的 isCJK 包含 0xAC00..=0xD7A3（韩文音节），本项目需要对齐。
    let tokens = estimate_text_tokens(Some("gemini-1.5-flash"), "가나다");
    // 3 chars * 0.68 => 2.04 -> ceil => 3
    assert_eq!(tokens, 3);
}

#[test]
fn estimate_tokens_treats_percent_as_url_delim() {
    // new-api 的 URLDelim 集合包含 '%'
    let tokens = estimate_text_tokens(Some("claude-3-opus"), "%");
    // URLDelim 1.26 -> ceil => 2
    assert_eq!(tokens, 2);
}

#[test]
fn estimate_tokens_treats_plus_as_symbol() {
    // new-api 的 mathSymbols 不包含 '+'，应按 Symbol 计费。
    let tokens = estimate_text_tokens(Some("claude-3-opus"), "+");
    // Symbol 0.4 -> ceil => 1
    assert_eq!(tokens, 1);
}
