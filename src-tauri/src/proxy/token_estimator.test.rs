use super::*;

#[test]
fn estimate_tokens_for_claude_uses_heuristic() {
    let tokens = estimate_text_tokens(Some("claude-3-opus"), "a");
    // Claude word multiplier 1.13 -> ceil => 2
    assert_eq!(tokens, 2);
}
