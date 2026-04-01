use crate::proxy::config::KiroPreferredEndpoint;

#[derive(Clone, Debug)]
pub(crate) struct KiroEndpointConfig {
    pub(crate) url: String,
    pub(crate) origin: &'static str,
    pub(crate) amz_target: &'static str,
}

pub(crate) fn select_endpoints(
    preferred: Option<KiroPreferredEndpoint>,
    is_idc: bool,
    base_url_override: Option<&str>,
) -> Vec<KiroEndpointConfig> {
    let codewhisperer = build_codewhisperer_endpoint(base_url_override);
    let amazon_q = build_amazon_q_endpoint(base_url_override);
    // IDC auth must use CodeWhisperer origin/endpoint pairing.
    if is_idc {
        return vec![codewhisperer];
    }

    match preferred {
        Some(KiroPreferredEndpoint::Ide) => vec![codewhisperer, amazon_q],
        Some(KiroPreferredEndpoint::Cli) => vec![amazon_q, codewhisperer],
        None => vec![codewhisperer, amazon_q],
    }
}

fn build_codewhisperer_endpoint(base_url_override: Option<&str>) -> KiroEndpointConfig {
    KiroEndpointConfig {
        url: build_endpoint_url(
            base_url_override,
            "https://codewhisperer.us-east-1.amazonaws.com/generateAssistantResponse",
        ),
        origin: "AI_EDITOR",
        amz_target: "AmazonCodeWhispererStreamingService.GenerateAssistantResponse",
    }
}

fn build_amazon_q_endpoint(base_url_override: Option<&str>) -> KiroEndpointConfig {
    KiroEndpointConfig {
        url: build_endpoint_url(
            base_url_override,
            "https://q.us-east-1.amazonaws.com/generateAssistantResponse",
        ),
        origin: "CLI",
        amz_target: "AmazonQDeveloperStreamingService.SendMessage",
    }
}

fn build_endpoint_url(base_url_override: Option<&str>, default_url: &str) -> String {
    let Some(base_url) = base_url_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return default_url.to_string();
    };
    format!("{}/generateAssistantResponse", base_url.trim_end_matches('/'))
}
