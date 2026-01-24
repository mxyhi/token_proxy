pub(crate) struct SseEventParser {
    buffer: String,
    current_data: String,
}

impl SseEventParser {
    pub(crate) fn new() -> Self {
        Self {
            buffer: String::new(),
            current_data: String::new(),
        }
    }

    pub(crate) fn push_chunk<F: FnMut(String)>(&mut self, chunk: &[u8], mut on_event: F) {
        let text = String::from_utf8_lossy(chunk);
        self.buffer.push_str(&text);
        while let Some(pos) = self.buffer.find('\n') {
            let mut line = self.buffer[..pos].to_string();
            self.buffer.drain(..=pos);
            if line.ends_with('\r') {
                line.pop();
            }
            self.process_line(&line, &mut on_event);
        }
    }

    pub(crate) fn finish<F: FnMut(String)>(&mut self, mut on_event: F) {
        if !self.buffer.is_empty() {
            let mut buffer = std::mem::take(&mut self.buffer);
            if buffer.ends_with('\r') {
                buffer.pop();
            }
            self.process_line(&buffer, &mut on_event);
        }
        self.flush_event(&mut on_event);
    }

    fn process_line<F: FnMut(String)>(&mut self, line: &str, on_event: &mut F) {
        if line.is_empty() {
            self.flush_event(on_event);
            return;
        }
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim_start();
            if !self.current_data.is_empty() {
                self.current_data.push('\n');
            }
            self.current_data.push_str(data);
        }
    }

    fn flush_event<F: FnMut(String)>(&mut self, on_event: &mut F) {
        if self.current_data.is_empty() {
            return;
        }
        let data = std::mem::take(&mut self.current_data);
        let data = data.trim();
        if data.is_empty() {
            return;
        }
        on_event(data.to_string());
    }
}

