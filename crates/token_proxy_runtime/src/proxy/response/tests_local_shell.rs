use super::*;
use crate::proxy::token_rate::RequestTokenTracker;

fn event(value: Value) -> Result<Bytes, std::io::Error> {
    Ok(Bytes::from(format!("data: {value}\n\n")))
}

async fn shell_stream(arguments: &[&str], finish: Option<&str>) -> (Vec<Value>, SqlitePool) {
    let (log, mut context, pool) = setup_responses_stream().await;
    let request = json!({"tools":[{"type":"shell","environment":{"type":"local"}}]});
    let name = token_proxy_protocol::tool_identity::local_shell::bridge_name(&request).unwrap();
    context.client_request_body = Some(Bytes::from(request.to_string()));
    let mut chunks = vec![event(
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"shell_a","function":{"name":name,"arguments":""}}]}}]}),
    )];
    chunks.extend(arguments.iter().map(|arguments| event(json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":arguments}}]}}]}))));
    if let Some(reason) = finish {
        chunks.push(event(
            json!({"choices":[{"delta":{},"finish_reason":reason}]}),
        ));
    }
    chunks.push(event(
        json!({"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}),
    ));
    chunks.push(Ok(Bytes::from("data: [DONE]\n\n")));
    let stream = stream_chat_to_responses(
        futures_util::stream::iter(chunks),
        context,
        log,
        RequestTokenTracker::disabled(),
    );
    let values = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|chunk| parse_sse_json(&chunk.unwrap()))
        .collect();
    (values, pool)
}

#[test]
fn local_shell_stream_buffers_fragments_and_delivers_validated_actions() {
    run_async(async {
        let (values, _) = shell_stream(
            &[
                "{\"commands\":[\"p",
                "wd\",\"ls\"],\"timeout_ms\":1000,",
                "\"max_output_length\":4096}",
            ],
            Some("tool_calls"),
        )
        .await;
        assert!(!values.iter().any(|value| value["type"]
            .as_str()
            .unwrap_or("")
            .contains("function_call")));
        let added = values
            .iter()
            .find(|value| value["type"] == "response.output_item.added")
            .unwrap();
        assert_eq!(added["item"]["type"], "shell_call");
        assert_eq!(added["item"]["action"]["commands"], json!(["pwd", "ls"]));
        assert_eq!(added["item"]["action"]["timeout_ms"], 1000);
        let done = values
            .iter()
            .find(|value| value["type"] == "response.output_item.done")
            .unwrap();
        assert_eq!(added["item"]["id"], done["item"]["id"]);
        assert_eq!(done["item"]["call_id"], "shell_a");
        assert_eq!(
            values
                .iter()
                .filter(|v| v["type"] == "response.shell_call_command.added")
                .count(),
            2
        );
        assert_eq!(
            values
                .iter()
                .filter(|v| v["type"] == "response.shell_call_command.done")
                .count(),
            2
        );
        assert!(values
            .windows(2)
            .all(|pair| pair[0]["sequence_number"].as_u64() < pair[1]["sequence_number"].as_u64()));
        let terminal = values.last().unwrap();
        assert_eq!(terminal["type"], "response.completed");
        assert_eq!(terminal["response"]["usage"]["total_tokens"], 5);
        assert_eq!(terminal["response"]["output"][0], done["item"]);
    });
}

#[test]
fn local_shell_stream_never_delivers_invalid_or_truncated_action() {
    run_async(async {
        for (arguments, finish) in [
            (vec!["{\"commands\":[]}"], Some("tool_calls")),
            (
                vec!["{\"commands\":[\"pwd\"],\"timeout_ms\":0}"],
                Some("tool_calls"),
            ),
            (vec!["{\"commands\":["], Some("tool_calls")),
            (vec!["{\"commands\":[\"pwd\"]}"], None),
        ] {
            let (values, pool) = shell_stream(&arguments, finish).await;
            assert!(values
                .iter()
                .any(|value| value["type"] == "response.failed"));
            assert!(!values
                .iter()
                .any(|value| value["item"]["type"] == "shell_call"
                    || value["item"]["type"] == "function_call"));
            assert!(!values.iter().any(|value| value["type"]
                .as_str()
                .unwrap_or("")
                .contains("shell_call_command")));
            for _ in 0..50 {
                let errors: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM request_logs WHERE status=502 AND response_error IS NOT NULL").fetch_one(&pool).await.unwrap();
                if errors == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let errors: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM request_logs WHERE status=502 AND response_error IS NOT NULL",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(errors, 1);
        }
    });
}
