use super::*;
use serde_json::json;

fn request(items: Value) -> ReplayableBody {
    ReplayableBody::from_bytes(serde_json::to_vec(&json!({"input":items})).unwrap().into())
}
fn rejection(message: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"error":{"code":"invalid_encrypted_content","message":message}}))
        .unwrap()
}
#[test]
fn codex_repair_removes_only_uniquely_rejected_item_and_shares_budget() {
    let initial = request(json!([
        {"type":"reasoning","encrypted_content":"abcd-middle-wxyz"},
        {"type":"reasoning","encrypted_content":"keep-ciphertext"},
        {"type":"message","content":"keep"}
    ]));
    let error=rejection("The encrypted content abcd...wxyz could not be verified. Reason: Encrypted content could not be decrypted or parsed.");
    let mut state = RequestRepairState::new(&initial);
    let repaired = repair_codex_encrypted_body(&initial, &error, &mut state)
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_slice(repaired.body.as_bytes()).unwrap();
    assert_eq!(value["input"].as_array().unwrap().len(), 2);
    assert_eq!(value["input"][0]["encrypted_content"], "keep-ciphertext");
    assert!(repair_codex_encrypted_body(&initial, &error, &mut state)
        .unwrap()
        .is_none());
}
#[test]
fn codex_repair_rejects_ambiguous_or_unsafe_history() {
    for items in [
        json!([{"type":"reasoning","encrypted_content":"one"},{"type":"reasoning","encrypted_content":"two"}]),
        json!([{"type":"reasoning","encrypted_content":"one"},{"type":"compaction","encrypted_content":"keep"}]),
        json!([{"type":"reasoning","id":"duplicate","encrypted_content":"one"},{"type":"message","id":"duplicate"}]),
        json!([{"type":"reasoning","encrypted_content":"abcd-one-wxyz"},{"type":"reasoning","encrypted_content":"abcd-two-wxyz"}]),
    ] {
        let initial = request(items);
        for error in [rejection("The encrypted content could not be verified."),rejection("The encrypted content abcd...wxyz could not be verified. Reason: Encrypted content could not be decrypted or parsed.")] {
            let mut state=RequestRepairState::new(&initial);
            assert!(repair_codex_encrypted_body(&initial,&error,&mut state).unwrap().is_none());
        }
    }
}
#[test]
fn codex_repair_requires_unambiguous_structured_rejection() {
    let initial = request(json!([{"type":"reasoning","encrypted_content":"one"}]));
    for error in [
        br#"{"error":{"code":"invalid_encrypted_content","code":"other","message":"The encrypted content could not be verified."}}"#.as_slice(),
        br#"{"error":{"code":"invalid_encrypted_content","message":"please remove everything"}}"#.as_slice(),
        br#"{"code":"invalid_encrypted_content","error":{"code":"invalid_encrypted_content","message":"The encrypted content could not be verified."}}"#.as_slice(),
    ] {
        let mut state=RequestRepairState::new(&initial);
        assert!(repair_codex_encrypted_body(&initial,error,&mut state).unwrap().is_none());
    }
}
