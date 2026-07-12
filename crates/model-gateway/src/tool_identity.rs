use crate::responses::GatewayTool;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use thiserror::Error;

const HASH_PREFIX_BYTES: usize = 8;
const MAX_WIRE_NAME_BYTES: usize = 64;
const MAX_READABLE_BYTES: usize = 44;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolNameMap {
    canonical_to_wire: BTreeMap<String, String>,
    advertised_to_wire: BTreeMap<String, String>,
    wire_to_canonical: BTreeMap<String, String>,
    wire_to_advertised: BTreeMap<String, String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ToolNameMapError {
    #[error("duplicate canonical tool id")]
    DuplicateCanonicalId,
    #[error("duplicate advertised provider tool id")]
    DuplicateAdvertisedId,
    #[error("provider alias is missing canonical tool entry")]
    MissingCanonicalEntry,
    #[error("provider tool name collision")]
    ProviderNameCollision,
    #[error("invalid provider tool name")]
    InvalidProviderName,
}

impl ToolNameMap {
    pub fn from_tools(tools: &[GatewayTool]) -> Result<Self, ToolNameMapError> {
        Self::from_tools_with(tools, wire_name_for)
    }

    fn from_tools_with(
        tools: &[GatewayTool],
        encode: impl Fn(&str) -> String,
    ) -> Result<Self, ToolNameMapError> {
        let mut canonical_to_wire = BTreeMap::new();
        let mut advertised_to_wire = BTreeMap::new();
        let mut advertised_is_canonical = BTreeMap::new();
        let mut wire_to_canonical = BTreeMap::new();
        let mut wire_to_advertised = BTreeMap::new();

        for tool in tools {
            let advertised = tool.advertised_name();
            let is_canonical = advertised == tool.id;
            if let Some(existing_is_canonical) = advertised_is_canonical.get(advertised) {
                return Err(if is_canonical && *existing_is_canonical {
                    ToolNameMapError::DuplicateCanonicalId
                } else {
                    ToolNameMapError::DuplicateAdvertisedId
                });
            }
            let wire = encode(advertised);
            validate_wire_name(&wire)?;
            if wire_to_canonical.contains_key(&wire) {
                return Err(ToolNameMapError::ProviderNameCollision);
            }
            advertised_is_canonical.insert(advertised.to_string(), is_canonical);
            advertised_to_wire.insert(advertised.to_string(), wire.clone());
            wire_to_canonical.insert(wire.clone(), tool.id.clone());
            wire_to_advertised.insert(wire.clone(), advertised.to_string());
            if is_canonical {
                canonical_to_wire.insert(tool.id.clone(), wire);
            }
        }

        if tools
            .iter()
            .any(|tool| !canonical_to_wire.contains_key(&tool.id))
        {
            return Err(ToolNameMapError::MissingCanonicalEntry);
        }

        Ok(Self {
            canonical_to_wire,
            advertised_to_wire,
            wire_to_canonical,
            wire_to_advertised,
        })
    }

    pub fn wire_name(&self, canonical: &str) -> Option<&str> {
        self.canonical_to_wire.get(canonical).map(String::as_str)
    }

    pub fn canonical_name(&self, wire: &str) -> Option<&str> {
        self.wire_to_canonical.get(wire).map(String::as_str)
    }

    pub fn wire_name_for_tool(&self, tool: &GatewayTool) -> Option<&str> {
        self.advertised_to_wire
            .get(tool.advertised_name())
            .map(String::as_str)
    }

    pub fn advertised_name(&self, wire: &str) -> Option<&str> {
        self.wire_to_advertised.get(wire).map(String::as_str)
    }

    #[cfg(test)]
    pub(crate) fn from_tools_with_test_encoder(
        tools: &[GatewayTool],
        encode: impl Fn(&str) -> String,
    ) -> Result<Self, ToolNameMapError> {
        Self::from_tools_with(tools, encode)
    }
}

fn wire_name_for(canonical: &str) -> String {
    let local_name = canonical.rsplit('/').next().unwrap_or(canonical);
    let readable: String = local_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .take(MAX_READABLE_BYTES)
        .collect();
    let readable = if readable.is_empty() {
        "tool".to_string()
    } else {
        readable
    };
    let digest = Sha256::digest(canonical.as_bytes());
    let hash = hex::encode(&digest[..HASH_PREFIX_BYTES]);
    format!("ga_{hash}_{readable}")
}

fn validate_wire_name(wire: &str) -> Result<(), ToolNameMapError> {
    if !wire.is_empty()
        && wire.len() <= MAX_WIRE_NAME_BYTES
        && wire
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        Ok(())
    } else {
        Err(ToolNameMapError::InvalidProviderName)
    }
}
