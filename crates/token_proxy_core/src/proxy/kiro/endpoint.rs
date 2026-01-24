use crate::proxy::config::KiroPreferredEndpoint;

#[derive(Clone, Copy, Debug)]
pub(crate) struct KiroEndpointConfig {
    pub(crate) url: &'static str,
    pub(crate) origin: &'static str,
    pub(crate) amz_target: &'static str,
}

const CODEWHISPERER_ENDPOINT: KiroEndpointConfig = KiroEndpointConfig {
    url: "https://codewhisperer.us-east-1.amazonaws.com/generateAssistantResponse",
    origin: "AI_EDITOR",
    amz_target: "AmazonCodeWhispererStreamingService.GenerateAssistantResponse",
};

const AMAZON_Q_ENDPOINT: KiroEndpointConfig = KiroEndpointConfig {
    url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse",
    origin: "CLI",
    amz_target: "AmazonQDeveloperStreamingService.SendMessage",
};

pub(crate) fn select_endpoints(
    preferred: Option<KiroPreferredEndpoint>,
    is_idc: bool,
) -> Vec<KiroEndpointConfig> {
    // IDC auth must use CodeWhisperer origin/endpoint pairing.
    if is_idc {
        return vec![CODEWHISPERER_ENDPOINT];
    }

    match preferred {
        Some(KiroPreferredEndpoint::Ide) => vec![CODEWHISPERER_ENDPOINT, AMAZON_Q_ENDPOINT],
        Some(KiroPreferredEndpoint::Cli) => vec![AMAZON_Q_ENDPOINT, CODEWHISPERER_ENDPOINT],
        None => vec![CODEWHISPERER_ENDPOINT, AMAZON_Q_ENDPOINT],
    }
}
