use std::sync::OnceLock;

use tiktoken_rs::{cl100k_base, o200k_base, CoreBPE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenProvider {
    OpenAI,
    Gemini,
    Claude,
}

#[derive(Clone, Copy)]
struct Multipliers {
    word: f64,
    number: f64,
    cjk: f64,
    symbol: f64,
    math_symbol: f64,
    url_delim: f64,
    at_sign: f64,
    emoji: f64,
    newline: f64,
    space: f64,
    base_pad: f64,
}

const MULTIPLIERS_OPENAI: Multipliers = Multipliers {
    word: 1.02,
    number: 1.55,
    cjk: 0.85,
    symbol: 0.4,
    math_symbol: 2.68,
    url_delim: 1.0,
    at_sign: 2.0,
    emoji: 2.12,
    newline: 0.5,
    space: 0.42,
    base_pad: 0.0,
};

const MULTIPLIERS_GEMINI: Multipliers = Multipliers {
    word: 1.15,
    number: 2.8,
    cjk: 0.68,
    symbol: 0.38,
    math_symbol: 1.05,
    url_delim: 1.2,
    at_sign: 2.5,
    emoji: 1.08,
    newline: 1.15,
    space: 0.2,
    base_pad: 0.0,
};

const MULTIPLIERS_CLAUDE: Multipliers = Multipliers {
    word: 1.13,
    number: 1.63,
    cjk: 1.21,
    symbol: 0.4,
    math_symbol: 4.52,
    url_delim: 1.26,
    at_sign: 2.82,
    emoji: 2.6,
    newline: 0.89,
    space: 0.39,
    base_pad: 0.0,
};

pub(crate) fn provider_for_model(model: Option<&str>) -> TokenProvider {
    let Some(model) = model else {
        return TokenProvider::OpenAI;
    };
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.contains("gemini") {
        return TokenProvider::Gemini;
    }
    if normalized.contains("claude") {
        return TokenProvider::Claude;
    }
    TokenProvider::OpenAI
}

pub(crate) fn estimate_text_tokens(model: Option<&str>, text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let provider = provider_for_model(model);
    if provider == TokenProvider::OpenAI {
        return estimate_text_tokens_openai(model, text);
    }
    estimate_text_tokens_by_provider(provider, text)
}

fn estimate_text_tokens_openai(model: Option<&str>, text: &str) -> u64 {
    let bpe = bpe_for_model(model);
    bpe.encode_with_special_tokens(text).len() as u64
}

fn estimate_text_tokens_by_provider(provider: TokenProvider, text: &str) -> u64 {
    let multipliers = match provider {
        TokenProvider::OpenAI => MULTIPLIERS_OPENAI,
        TokenProvider::Gemini => MULTIPLIERS_GEMINI,
        TokenProvider::Claude => MULTIPLIERS_CLAUDE,
    };

    // 以字符类别估算 token 数，复刻 new-api 的启发式逻辑。
    let mut count = 0.0f64;
    let mut current_word_type: Option<WordType> = None;

    for ch in text.chars() {
        if ch.is_whitespace() {
            current_word_type = None;
            if ch == '\n' || ch == '\t' {
                count += multipliers.newline;
            } else {
                count += multipliers.space;
            }
            continue;
        }

        if is_cjk(ch) {
            current_word_type = None;
            count += multipliers.cjk;
            continue;
        }

        if is_emoji(ch) {
            current_word_type = None;
            count += multipliers.emoji;
            continue;
        }

        if is_latin_or_number(ch) {
            let new_type = if ch.is_ascii_digit() || ch.is_numeric() {
                WordType::Number
            } else {
                WordType::Latin
            };

            if current_word_type.is_none() || current_word_type != Some(new_type) {
                count += match new_type {
                    WordType::Latin => multipliers.word,
                    WordType::Number => multipliers.number,
                };
                current_word_type = Some(new_type);
            }
            continue;
        }

        current_word_type = None;
        if is_math_symbol(ch) {
            count += multipliers.math_symbol;
        } else if ch == '@' {
            count += multipliers.at_sign;
        } else if is_url_delim(ch) {
            count += multipliers.url_delim;
        } else {
            count += multipliers.symbol;
        }
    }

    let total = count.ceil() + multipliers.base_pad;
    total as u64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WordType {
    Latin,
    Number,
}

fn bpe_for_model(model: Option<&str>) -> &'static CoreBPE {
    if matches_o200k(model) {
        static O200K: OnceLock<CoreBPE> = OnceLock::new();
        return O200K.get_or_init(|| {
            o200k_base().unwrap_or_else(|_| cl100k_base().expect("cl100k_base"))
        });
    }

    static CL100K: OnceLock<CoreBPE> = OnceLock::new();
    CL100K.get_or_init(|| cl100k_base().expect("cl100k_base"))
}

fn matches_o200k(model: Option<&str>) -> bool {
    let Some(model) = model else {
        return false;
    };
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    normalized.starts_with("o1")
        || normalized.starts_with("o3")
        || normalized.starts_with("o4")
        || normalized.starts_with("gpt-4o")
        || normalized.starts_with("gpt-4.1")
}

fn is_latin_or_number(ch: char) -> bool {
    if ch.is_ascii_alphanumeric() {
        return true;
    }
    ch.is_alphanumeric()
}

fn is_cjk(ch: char) -> bool {
    let code = ch as u32;
    matches!(
        code,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2B820..=0x2CEAF
            | 0x2CEB0..=0x2EBEF
            | 0x30000..=0x3134F
    )
}

fn is_emoji(ch: char) -> bool {
    let code = ch as u32;
    matches!(
        code,
        0x1F300..=0x1F5FF
            | 0x1F600..=0x1F64F
            | 0x1F680..=0x1F6FF
            | 0x1F700..=0x1F77F
            | 0x1F780..=0x1F7FF
            | 0x1F800..=0x1F8FF
            | 0x1F900..=0x1F9FF
            | 0x1FA00..=0x1FAFF
            | 0x2600..=0x26FF
            | 0x2700..=0x27BF
    )
}

fn is_math_symbol(ch: char) -> bool {
    let code = ch as u32;
    matches!(
        code,
        0x2200..=0x22FF | 0x27C0..=0x27EF | 0x2980..=0x29FF | 0x2A00..=0x2AFF
            | 0x2190..=0x21FF | 0x2B00..=0x2BFF
    ) || matches!(ch, '+' | '-' | '*' | '/' | '=' | '^' | '%')
}

fn is_url_delim(ch: char) -> bool {
    matches!(
        ch,
        ':' | '/' | '?' | '#' | '[' | ']' | '!' | '$' | '&' | '\''
            | '(' | ')' | '*' | '+' | ',' | ';' | '='
    )
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "token_estimator.test.rs"]
mod tests;
