use axum::http::{
    header::{ACCEPT, CONTENT_TYPE, USER_AGENT},
    HeaderMap, HeaderName, HeaderValue,
};

use crate::proxy::http;

const KIRO_REQUEST_CONTENT_TYPE: &str = "application/x-amz-json-1.0";
const KIRO_REQUEST_ACCEPT: &str = "*/*";
const KIRO_AGENT_MODE_IDC: &str = "spec";
const KIRO_AGENT_MODE_DEFAULT: &str = "vibe";
const KIRO_OPT_OUT: &str = "true";
const KIRO_SDK_REQUEST: &str = "attempt=1; max=3";
const KIRO_USER_AGENT_IDC: &str = "aws-sdk-js/1.0.18 ua/2.1 os/darwin#25.0.0 lang/js md/nodejs#20.16.0 api/codewhispererstreaming#1.0.18 m/E KiroIDE-0.2.13-66c23a8c5d15afabec89ef9954ef52a119f10d369df04d548fc6c1eac694b0d1";
const KIRO_USER_AGENT_IDC_AMZ: &str =
    "aws-sdk-js/1.0.18 KiroIDE-0.2.13-66c23a8c5d15afabec89ef9954ef52a119f10d369df04d548fc6c1eac694b0d1";
const KIRO_USER_AGENT_DEFAULT: &str = "aws-sdk-rust/1.3.9 os/macos lang/rust/1.87.0";
const KIRO_USER_AGENT_DEFAULT_AMZ: &str =
    "aws-sdk-rust/1.3.9 ua/2.1 api/ssooidc/1.88.0 os/macos lang/rust/1.87.0 m/E app/AmazonQ-For-CLI";

const HEADER_AMZ_TARGET: HeaderName = HeaderName::from_static("x-amz-target");
const HEADER_AMZ_USER_AGENT: HeaderName = HeaderName::from_static("x-amz-user-agent");
const HEADER_AMZ_SDK_REQUEST: HeaderName = HeaderName::from_static("amz-sdk-request");
const HEADER_AMZ_SDK_INVOCATION_ID: HeaderName = HeaderName::from_static("amz-sdk-invocation-id");
const HEADER_KIRO_AGENT_MODE: HeaderName = HeaderName::from_static("x-amzn-kiro-agent-mode");
const HEADER_KIRO_OPTOUT: HeaderName = HeaderName::from_static("x-amzn-codewhisperer-optout");

pub(super) fn build_kiro_headers(
    access_token: &str,
    amz_target: &str,
    is_idc: bool,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(KIRO_REQUEST_CONTENT_TYPE));
    headers.insert(ACCEPT, HeaderValue::from_static(KIRO_REQUEST_ACCEPT));
    if let Ok(value) = HeaderValue::from_str(amz_target) {
        headers.insert(HEADER_AMZ_TARGET, value);
    }
    headers.insert(HEADER_AMZ_SDK_REQUEST, HeaderValue::from_static(KIRO_SDK_REQUEST));
    if let Ok(value) = HeaderValue::from_str(&crate::proxy::kiro::utils::random_uuid()) {
        headers.insert(HEADER_AMZ_SDK_INVOCATION_ID, value);
    }
    headers.insert(
        HEADER_KIRO_AGENT_MODE,
        HeaderValue::from_static(if is_idc {
            KIRO_AGENT_MODE_IDC
        } else {
            KIRO_AGENT_MODE_DEFAULT
        }),
    );
    headers.insert(HEADER_KIRO_OPTOUT, HeaderValue::from_static(KIRO_OPT_OUT));
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(if is_idc {
            KIRO_USER_AGENT_IDC
        } else {
            KIRO_USER_AGENT_DEFAULT
        }),
    );
    headers.insert(
        HEADER_AMZ_USER_AGENT,
        HeaderValue::from_static(if is_idc {
            KIRO_USER_AGENT_IDC_AMZ
        } else {
            KIRO_USER_AGENT_DEFAULT_AMZ
        }),
    );
    if let Some(auth) = http::bearer_header(access_token) {
        headers.insert(axum::http::header::AUTHORIZATION, auth);
    }
    headers
}
