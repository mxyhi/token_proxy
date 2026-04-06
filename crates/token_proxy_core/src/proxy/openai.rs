const OPENAI_RESPONSES_ROOT: &str = "/v1/responses";
const OPENAI_CHAT_COMPLETIONS_ROOT: &str = "/v1/chat/completions";

const OPENAI_NATIVE_PREFIX_ROOTS: &[&str] = &[
    "/v1/assistants",
    "/v1/batches",
    "/v1/chatkit",
    "/v1/completions",
    "/v1/containers",
    "/v1/conversations",
    "/v1/embeddings",
    "/v1/evals",
    "/v1/files",
    "/v1/moderations",
    "/v1/skills",
    "/v1/threads",
    "/v1/uploads",
    "/v1/vector_stores",
    "/v1/videos",
];

pub(crate) fn is_openai_responses_resource_path(path: &str) -> bool {
    let path = strip_query(path);
    path.starts_with(&format!("{OPENAI_RESPONSES_ROOT}/"))
}

pub(crate) fn is_openai_native_resource_path(path: &str) -> bool {
    let path = strip_query(path);
    if path.starts_with(&format!("{OPENAI_CHAT_COMPLETIONS_ROOT}/")) {
        return true;
    }
    if OPENAI_NATIVE_PREFIX_ROOTS
        .iter()
        .any(|root| matches_root(path, root))
    {
        return true;
    }

    path.starts_with("/v1/audio/")
        || path.starts_with("/v1/fine_tuning/")
        || path.starts_with("/v1/images/")
        || path.starts_with("/v1/realtime/")
}

fn matches_root(path: &str, root: &str) -> bool {
    path == root || path.starts_with(&format!("{root}/"))
}

fn strip_query(path: &str) -> &str {
    path.split_once('?').map(|(path, _)| path).unwrap_or(path)
}
