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
fn codex_repair_matches_azure_rejected_reasoning_id_exactly() {
    let items = json!([
        {"type":"reasoning","id":"rs_keep","encrypted_content":"keep-ciphertext"},
        {"type":"reasoning","id":"rs_rejected","encrypted_content":"rejected-ciphertext"},
        {"type":"message","role":"user","content":"keep"},
        {"type":"function_call","namespace":"team","call_id":"call_1","name":"exec","arguments":"{}"},
        {"type":"function_call_output","call_id":"call_1","output":"keep"}
    ]);
    let initial = request(items.clone());
    let error = rejection("The encrypted content for item rs_rejected could not be verified. Reason: Encrypted content could not be decrypted or parsed.");
    let mut state = RequestRepairState::new(&initial);
    let repaired = repair_codex_encrypted_body(&initial, &error, &mut state)
        .unwrap()
        .expect("explicitly rejected reasoning id is recoverable");
    let value: Value = serde_json::from_slice(repaired.body.as_bytes()).unwrap();
    let mut expected = items.as_array().unwrap().clone();
    expected.remove(1);
    assert_eq!(value["input"], json!(expected));
    assert!(repair_codex_encrypted_body(&initial, &error, &mut state)
        .unwrap()
        .is_none());
}

#[test]
fn codex_repair_rejects_missing_duplicate_or_non_reasoning_item_ids() {
    let error = rejection("The encrypted content for item rs_rejected could not be verified. Reason: Encrypted content could not be decrypted or parsed.");
    for items in [
        json!([{"type":"reasoning","id":"rs_other","encrypted_content":"one"}]),
        json!([{"type":"reasoning","id":"rs_rejected","encrypted_content":"one"},{"type":"message","id":"rs_rejected"}]),
        json!([{"type":"message","id":"rs_rejected","encrypted_content":"one"}]),
        json!([{"type":"reasoning","id":"rs_rejected"}]),
        json!([{"type":"reasoning","id":"rs_rejected","encrypted_content":"one"},{"type":"compaction","encrypted_content":"keep"}]),
    ] {
        let initial = request(items);
        let mut state = RequestRepairState::new(&initial);
        assert!(repair_codex_encrypted_body(&initial, &error, &mut state)
            .unwrap()
            .is_none());
    }
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

#[test]
fn codex_repair_caps_multi_item_history_loss() {
    let items = (0..65)
        .map(|index| {
            json!({
                "type": "reasoning",
                "id": format!("rs_{index}"),
                "encrypted_content": format!("cipher-{index}")
            })
        })
        .collect::<Vec<_>>();
    let mut body = request(json!(items));
    let mut state = RequestRepairState::new(&body);
    for index in 0..64 {
        let error = rejection(&format!(
            "The encrypted content for item rs_{index} could not be verified. Reason: Encrypted content could not be decrypted or parsed."
        ));
        body = repair_codex_encrypted_body(&body, &error, &mut state)
            .unwrap()
            .expect("repair within bounded loss")
            .body;
    }
    let error = rejection(
        "The encrypted content for item rs_64 could not be verified. Reason: Encrypted content could not be decrypted or parsed.",
    );
    assert!(repair_codex_encrypted_body(&body, &error, &mut state)
        .unwrap()
        .is_none());
}

#[test]
fn azuregpt_repair_allows_long_history_migration() {
    let items = (0..257)
        .map(|index| {
            json!({
                "type": "reasoning",
                "id": format!("rs_{index}"),
                "encrypted_content": format!("cipher-{index}"),
                "summary": []
            })
        })
        .collect::<Vec<_>>();
    let mut body = request(json!(items));
    let mut state = RequestRepairState::new_for_provider(&body, "azuregpt");
    for index in 0..256 {
        let error = rejection(&format!(
            "The encrypted content for item rs_{index} could not be verified. Reason: Encrypted content could not be decrypted or parsed."
        ));
        body = repair_codex_encrypted_body(&body, &error, &mut state)
            .unwrap()
            .expect("Azure GPT should repair the rejected item")
            .body;
    }
    let error = rejection(
        "The encrypted content for item rs_256 could not be verified. Reason: Encrypted content could not be decrypted or parsed.",
    );
    assert!(repair_codex_encrypted_body(&body, &error, &mut state)
        .unwrap()
        .is_none());
}
