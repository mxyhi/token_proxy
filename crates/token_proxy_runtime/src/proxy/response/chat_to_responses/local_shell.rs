use axum::body::Bytes;
use serde_json::{json, Value};
use token_proxy_protocol::tool_identity::local_shell;

use super::{function_call_item, ChatToResponsesState};

impl<S, E> ChatToResponsesState<S>
where
    S: futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    pub(super) fn validate_shell_calls(&self) -> Result<(), String> {
        let Some(name) = self.shell_bridge_name.as_deref() else {
            return Ok(());
        };
        for call in self
            .function_calls
            .iter()
            .flatten()
            .filter(|call| call.name == name)
        {
            local_shell::parse_action(&call.arguments)?;
        }
        Ok(())
    }

    pub(super) fn shell_output_item(&self, mut item: Value) -> Value {
        if let Some(name) = self.shell_bridge_name.as_deref() {
            // push_done 在发布任何 shell item 前验证了全部 action；这里只还原同一快照。
            local_shell::restore_item(&mut item, name)
                .expect("shell action validated before delivery");
        }
        item
    }

    pub(super) fn push_shell_item_added(&mut self, item: Value, output_index: u64) {
        let commands = item["action"]["commands"]
            .as_array()
            .expect("validated commands")
            .clone();
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::super::responses_event_sse(json!({
            "type":"response.output_item.added", "output_index":output_index,
            "item":item, "sequence_number":sequence_number
        })));
        for (command_index, command) in commands.into_iter().enumerate() {
            let sequence_number = self.next_sequence_number();
            self.out.push_back(super::super::responses_event_sse(json!({
                "type":"response.shell_call_command.added", "output_index":output_index,
                "command_index":command_index, "command":command, "sequence_number":sequence_number
            })));
        }
    }

    pub(super) fn push_shell_done_events(&mut self, item: Value, output_index: u64) {
        for (command_index, command) in item["action"]["commands"]
            .as_array()
            .expect("validated commands")
            .iter()
            .enumerate()
        {
            let sequence_number = self.next_sequence_number();
            self.out.push_back(super::super::responses_event_sse(json!({
                "type":"response.shell_call_command.done", "output_index":output_index,
                "command_index":command_index, "command":command, "sequence_number":sequence_number
            })));
        }
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::super::responses_event_sse(json!({
            "type":"response.output_item.done", "output_index":output_index,
            "item":item, "sequence_number":sequence_number
        })));
    }

    pub(super) fn emit_shell_call_if_ready(&mut self, call_index: usize, terminal: bool) -> bool {
        // 不交付分片 JSON 或未完成 action，防止客户端提前执行局部 commands。
        if !terminal {
            return false;
        }
        let call = self.function_calls[call_index]
            .as_mut()
            .expect("shell call");
        call.item_added = true;
        let output_index = call.output_index;
        let item = function_call_item(
            &call.id,
            "in_progress",
            &call.call_id,
            &call.name,
            &call.arguments,
            call.provider_specific_fields.as_ref(),
        );
        let item = self.shell_output_item(item);
        self.push_shell_item_added(item, output_index);
        true
    }
}
