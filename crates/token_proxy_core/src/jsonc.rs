/// JSONC utilities: strip `//` and `/* */` comments, and remove trailing commas.
///
/// NOTE:
/// - We intentionally keep this minimal and dependency-free.
/// - This will remove comments even if the file extension is `.json`.
pub fn sanitize_jsonc(contents: &str) -> String {
    let without_comments = strip_jsonc_comments(contents);
    strip_trailing_commas(&without_comments)
}

fn strip_jsonc_comments(contents: &str) -> String {
    let bytes = contents.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escape = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            consume_string_byte(byte, &mut in_string, &mut escape, &mut output);
            index += 1;
            continue;
        }

        if byte == b'"' {
            in_string = true;
            output.push(byte);
            index += 1;
            continue;
        }

        if let Some(next_index) = try_skip_comment(bytes, index, &mut output) {
            index = next_index;
            continue;
        }

        output.push(byte);
        index += 1;
    }

    String::from_utf8(output).unwrap_or_default()
}

fn consume_string_byte(byte: u8, in_string: &mut bool, escape: &mut bool, output: &mut Vec<u8>) {
    output.push(byte);
    if *escape {
        *escape = false;
    } else if byte == b'\\' {
        *escape = true;
    } else if byte == b'"' {
        *in_string = false;
    }
}

fn try_skip_comment(bytes: &[u8], index: usize, output: &mut Vec<u8>) -> Option<usize> {
    if bytes[index] != b'/' || index + 1 >= bytes.len() {
        return None;
    }
    match bytes[index + 1] {
        b'/' => Some(skip_line_comment(bytes, index + 2, output)),
        b'*' => Some(skip_block_comment(bytes, index + 2, output)),
        _ => None,
    }
}

fn skip_line_comment(bytes: &[u8], mut index: usize, output: &mut Vec<u8>) -> usize {
    // Line comment: skip until newline, keep the newline for line numbers.
    while index < bytes.len() {
        let current = bytes[index];
        if current == b'\n' {
            output.push(b'\n');
            return index + 1;
        }
        index += 1;
    }
    index
}

fn skip_block_comment(bytes: &[u8], mut index: usize, output: &mut Vec<u8>) -> usize {
    // Block comment: preserve line breaks for better error positions.
    while index + 1 < bytes.len() {
        let current = bytes[index];
        if current == b'\n' {
            output.push(b'\n');
        }
        if current == b'*' && bytes[index + 1] == b'/' {
            return index + 2;
        }
        index += 1;
    }
    index
}

fn strip_trailing_commas(contents: &str) -> String {
    let bytes = contents.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escape = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            output.push(byte);
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        if byte == b'"' {
            in_string = true;
            output.push(byte);
            index += 1;
            continue;
        }

        if byte == b',' {
            let mut lookahead = index + 1;
            let mut should_skip = false;
            while lookahead < bytes.len() {
                let next = bytes[lookahead];
                if next == b' ' || next == b'\t' || next == b'\r' || next == b'\n' {
                    lookahead += 1;
                    continue;
                }
                if next == b'}' || next == b']' {
                    should_skip = true;
                }
                break;
            }
            if should_skip {
                index += 1;
                continue;
            }
        }

        output.push(byte);
        index += 1;
    }

    String::from_utf8(output).unwrap_or_default()
}

