use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndpointType {
    Responses,
    ChatCompletions,
    Completion,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProviderProfile {
    pub id: String,
    pub name: String,
    pub endpoint_type: EndpointType,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl ProviderProfile {
    pub fn endpoint_path(&self) -> &'static str {
        match self.endpoint_type {
            EndpointType::Responses => "/responses",
            EndpointType::ChatCompletions => "/chat/completions",
            EndpointType::Completion => "/completions",
        }
    }

    pub fn endpoint_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = self.endpoint_path().trim_start_matches('/');
        format!("{base}/{path}")
    }

    pub fn supports_tools(&self) -> bool {
        match self.endpoint_type {
            EndpointType::Responses | EndpointType::ChatCompletions => true,
            EndpointType::Completion => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_url_joins_base_and_path() {
        let profile = ProviderProfile {
            id: "local".into(),
            name: "Local".into(),
            endpoint_type: EndpointType::ChatCompletions,
            base_url: "http://localhost:11434/v1/".into(),
            model: "qwen".into(),
            api_key: None,
            headers: BTreeMap::new(),
        };

        assert_eq!(
            profile.endpoint_url(),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn completion_endpoint_does_not_support_tools() {
        let profile = ProviderProfile {
            id: "legacy".into(),
            name: "Legacy".into(),
            endpoint_type: EndpointType::Completion,
            base_url: "http://localhost:11434/v1".into(),
            model: "legacy-model".into(),
            api_key: None,
            headers: BTreeMap::new(),
        };

        assert!(!profile.supports_tools());
    }

    #[test]
    fn responses_endpoint_supports_tools() {
        let profile = ProviderProfile {
            id: "openai".into(),
            name: "OpenAI".into(),
            endpoint_type: EndpointType::Responses,
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-5.4".into(),
            api_key: None,
            headers: BTreeMap::new(),
        };

        assert!(profile.supports_tools());
    }

    #[test]
    fn chat_completions_endpoint_supports_tools() {
        let profile = ProviderProfile {
            id: "local".into(),
            name: "Local".into(),
            endpoint_type: EndpointType::ChatCompletions,
            base_url: "http://localhost:11434/v1".into(),
            model: "qwen".into(),
            api_key: None,
            headers: BTreeMap::new(),
        };

        assert!(profile.supports_tools());
    }
}
