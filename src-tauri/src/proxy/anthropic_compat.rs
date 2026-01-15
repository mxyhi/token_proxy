use axum::body::Bytes;

use super::http_client::ProxyHttpClients;

mod media;
mod request;
mod response;
mod tools;

pub(crate) async fn responses_request_to_anthropic(
    body: &Bytes,
    http_clients: &ProxyHttpClients,
) -> Result<Bytes, String> {
    request::responses_request_to_anthropic(body, http_clients).await
}

pub(crate) async fn anthropic_request_to_responses(
    body: &Bytes,
    http_clients: &ProxyHttpClients,
) -> Result<Bytes, String> {
    request::anthropic_request_to_responses(body, http_clients).await
}

pub(crate) fn responses_response_to_anthropic(
    body: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    response::responses_response_to_anthropic(body, model_hint)
}

pub(crate) fn anthropic_response_to_responses(body: &Bytes) -> Result<Bytes, String> {
    response::anthropic_response_to_responses(body)
}

#[cfg(test)]
mod tests;
