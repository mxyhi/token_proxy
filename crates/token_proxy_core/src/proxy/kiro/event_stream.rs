use std::io;

const MIN_FRAME_SIZE: usize = 16;
const MAX_FRAME_SIZE: usize = 10 << 20;

#[derive(Debug)]
pub(crate) struct EventStreamError {
    pub(crate) message: String,
}

#[derive(Debug, Clone)]
pub(crate) struct EventStreamMessage {
    pub(crate) event_type: String,
    pub(crate) payload: Vec<u8>,
}

pub(crate) struct EventStreamDecoder {
    buffer: Vec<u8>,
}

impl EventStreamDecoder {
    pub(crate) fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub(crate) fn push(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<EventStreamMessage>, EventStreamError> {
        self.buffer.extend_from_slice(chunk);
        self.decode_available()
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<EventStreamMessage>, EventStreamError> {
        self.decode_available()
    }

    fn decode_available(&mut self) -> Result<Vec<EventStreamMessage>, EventStreamError> {
        let mut out = Vec::new();
        loop {
            if self.buffer.len() < MIN_FRAME_SIZE {
                break;
            }
            let total_len = read_u32(&self.buffer[0..4]) as usize;
            let headers_len = read_u32(&self.buffer[4..8]) as usize;

            if total_len < MIN_FRAME_SIZE {
                return Err(EventStreamError {
                    message: "EventStream frame too small".to_string(),
                });
            }
            if total_len > MAX_FRAME_SIZE {
                return Err(EventStreamError {
                    message: "EventStream frame too large".to_string(),
                });
            }
            if self.buffer.len() < total_len {
                break;
            }

            let headers_start = 12;
            let headers_end = headers_start + headers_len;
            if headers_end > total_len {
                return Err(EventStreamError {
                    message: "EventStream header length invalid".to_string(),
                });
            }
            let payload_start = headers_end;
            if payload_start + 4 > total_len {
                return Err(EventStreamError {
                    message: "EventStream payload length invalid".to_string(),
                });
            }
            let payload_end = total_len - 4; // last 4 bytes are message CRC
            let headers = &self.buffer[headers_start..headers_end];
            let payload = self.buffer[payload_start..payload_end].to_vec();

            let event_type = parse_event_type(headers).unwrap_or_default();
            out.push(EventStreamMessage {
                event_type,
                payload,
            });

            self.buffer.drain(0..total_len);
        }
        Ok(out)
    }
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn parse_event_type(headers: &[u8]) -> Option<String> {
    let mut cursor = 0;
    while cursor < headers.len() {
        let name_len = *headers.get(cursor)? as usize;
        cursor += 1;
        if cursor + name_len > headers.len() {
            return None;
        }
        let name = std::str::from_utf8(&headers[cursor..cursor + name_len]).ok()?;
        cursor += name_len;
        let header_type = *headers.get(cursor)?;
        cursor += 1;

        let value = match header_type {
            0 | 1 => None,
            2 => {
                cursor += 1;
                None
            }
            3 => {
                cursor += 2;
                None
            }
            4 => {
                cursor += 4;
                None
            }
            5 | 8 => {
                cursor += 8;
                None
            }
            6 | 7 => {
                if cursor + 2 > headers.len() {
                    return None;
                }
                let len = u16::from_be_bytes([headers[cursor], headers[cursor + 1]]) as usize;
                cursor += 2;
                if cursor + len > headers.len() {
                    return None;
                }
                let bytes = &headers[cursor..cursor + len];
                cursor += len;
                if header_type == 7 {
                    Some(String::from_utf8_lossy(bytes).to_string())
                } else {
                    None
                }
            }
            9 => {
                cursor += 16;
                None
            }
            _ => return None,
        };

        if name == ":event-type" {
            return value;
        }
    }
    None
}

impl From<io::Error> for EventStreamError {
    fn from(err: io::Error) -> Self {
        EventStreamError {
            message: err.to_string(),
        }
    }
}
