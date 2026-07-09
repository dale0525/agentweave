use model_gateway::provider::{EndpointType, ProviderProfile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct StoredModelConfig {
    pub provider_id: String,
    pub provider_name: String,
    pub endpoint_type: EndpointType,
    pub base_url: String,
    pub model_name: String,
    pub secret_id: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl StoredModelConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.base_url.trim().is_empty() {
            return Err("base URL is required".into());
        }
        if self.model_name.trim().is_empty() {
            return Err("model name is required".into());
        }
        if let Some(header_name) = self
            .headers
            .keys()
            .find(|name| is_sensitive_header_name(name))
        {
            return Err(format!(
                "header `{header_name}` must not contain secrets; store API keys in secure storage"
            ));
        }
        Ok(())
    }

    pub fn to_provider_profile(&self, api_key: Option<String>) -> ProviderProfile {
        ProviderProfile {
            id: self.provider_id.clone(),
            name: self.provider_name.clone(),
            endpoint_type: self.endpoint_type,
            base_url: self.base_url.clone(),
            model: self.model_name.clone(),
            api_key,
            headers: self.headers.clone(),
        }
    }
}

fn is_sensitive_header_name(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "authorization" | "proxy-authorization") {
        return true;
    }

    let compact: String = normalized
        .chars()
        .filter(|ch| *ch != '-' && *ch != '_' && !ch.is_whitespace())
        .collect();
    compact.contains("apikey")
        || compact.contains("authtoken")
        || compact.contains("accesstoken")
        || compact.contains("apitoken")
        || compact.contains("sessiontoken")
        || compact.contains("token")
}

#[cfg(test)]
mod tests {
    use super::*;
    use model_gateway::provider::ProviderProfile;

    #[test]
    fn validates_required_fields() {
        let config = StoredModelConfig {
            provider_id: "local".into(),
            provider_name: "Local".into(),
            endpoint_type: EndpointType::ChatCompletions,
            base_url: "".into(),
            model_name: "".into(),
            secret_id: None,
            headers: BTreeMap::new(),
        };

        assert_eq!(config.validate().unwrap_err(), "base URL is required");
    }

    #[test]
    fn rejects_sensitive_authorization_headers() {
        let config = StoredModelConfig {
            provider_id: "remote".into(),
            provider_name: "Remote".into(),
            endpoint_type: EndpointType::Responses,
            base_url: "https://example.com".into(),
            model_name: "gpt-test".into(),
            secret_id: Some("secret-1".into()),
            headers: BTreeMap::from([("Authorization".into(), "Bearer secret".into())]),
        };

        assert_eq!(
            config.validate().unwrap_err(),
            "header `Authorization` must not contain secrets; store API keys in secure storage"
        );
    }

    #[test]
    fn rejects_api_key_style_headers_but_allows_custom_headers() {
        let config = StoredModelConfig {
            provider_id: "remote".into(),
            provider_name: "Remote".into(),
            endpoint_type: EndpointType::Responses,
            base_url: "https://example.com".into(),
            model_name: "gpt-test".into(),
            secret_id: Some("secret-1".into()),
            headers: BTreeMap::from([("X-API-Key".into(), "secret".into())]),
        };

        assert_eq!(
            config.validate().unwrap_err(),
            "header `X-API-Key` must not contain secrets; store API keys in secure storage"
        );

        let safe_config = StoredModelConfig {
            headers: BTreeMap::from([("X-Client-Version".into(), "android-1".into())]),
            ..config
        };

        assert!(safe_config.validate().is_ok());
    }

    #[test]
    fn rejects_token_bearing_header_names() {
        let base = StoredModelConfig {
            provider_id: "remote".into(),
            provider_name: "Remote".into(),
            endpoint_type: EndpointType::Responses,
            base_url: "https://example.com".into(),
            model_name: "gpt-test".into(),
            secret_id: Some("secret-1".into()),
            headers: BTreeMap::new(),
        };

        for header_name in [
            "X-Auth-Token",
            "X-Access-Token",
            "Api-Token",
            "Session-Token",
        ] {
            let config = StoredModelConfig {
                headers: BTreeMap::from([(header_name.into(), "secret".into())]),
                ..base.clone()
            };

            assert_eq!(
                config.validate().unwrap_err(),
                format!(
                    "header `{header_name}` must not contain secrets; store API keys in secure storage"
                )
            );
        }
    }

    #[test]
    fn provider_profile_uses_runtime_secret_without_persisting_it() {
        let config = StoredModelConfig {
            provider_id: "openai".into(),
            provider_name: "OpenAI".into(),
            endpoint_type: EndpointType::Responses,
            base_url: "https://api.openai.com/v1".into(),
            model_name: "gpt-5.4".into(),
            secret_id: Some("model.openai.default".into()),
            headers: BTreeMap::new(),
        };

        let profile = config.to_provider_profile(Some("sk-runtime".into()));
        let stored_json = serde_json::to_string(&config).unwrap();

        assert_eq!(
            profile,
            ProviderProfile {
                id: "openai".into(),
                name: "OpenAI".into(),
                endpoint_type: EndpointType::Responses,
                base_url: "https://api.openai.com/v1".into(),
                model: "gpt-5.4".into(),
                api_key: Some("sk-runtime".into()),
                headers: BTreeMap::new(),
            }
        );
        assert!(!stored_json.contains("sk-runtime"));
    }
}
