use model_gateway::provider::EndpointType;
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
