use std::{collections::HashMap, sync::Mutex};

use reqwest::{Client, ClientBuilder, Proxy};

pub(crate) struct ProxyHttpClients {
    direct: Client,
    by_proxy_url: Mutex<HashMap<String, Client>>,
}

impl ProxyHttpClients {
    pub(crate) fn new() -> Result<Self, String> {
        let direct = ClientBuilder::new()
            // 默认不走系统代理；仅当用户显式配置 proxy_url 时才走代理。
            .no_proxy()
            .build()
            .map_err(|err| format!("Failed to build direct HTTP client: {err}"))?;
        Ok(Self {
            direct,
            by_proxy_url: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn client_for_proxy_url(&self, proxy_url: Option<&str>) -> Result<Client, String> {
        let Some(proxy_url) = proxy_url
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            return Ok(self.direct.clone());
        };

        let mut guard = self
            .by_proxy_url
            .lock()
            .map_err(|_| "HTTP client pool is poisoned.".to_string())?;
        if let Some(existing) = guard.get(proxy_url) {
            return Ok(existing.clone());
        }

        let proxy = Proxy::all(proxy_url)
            .map_err(|_| "proxy_url is invalid or not supported.".to_string())?;
        let client = ClientBuilder::new()
            .proxy(proxy)
            .build()
            .map_err(|err| format!("Failed to build proxied HTTP client: {err}"))?;
        guard.insert(proxy_url.to_string(), client.clone());
        Ok(client)
    }
}
