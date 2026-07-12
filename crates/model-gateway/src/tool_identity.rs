use crate::responses::GatewayTool;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const HASH_PREFIX_BYTES: usize = 8;
const MAX_WIRE_NAME_BYTES: usize = 64;
const MAX_READABLE_BYTES: usize = 44;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolNameMap {
    canonical_to_wire: BTreeMap<String, String>,
    wire_to_canonical: BTreeMap<String, String>,
}

impl ToolNameMap {
    pub fn from_tools(tools: &[GatewayTool]) -> anyhow::Result<Self> {
        Self::from_tools_with(tools, wire_name_for)
    }

    fn from_tools_with(
        tools: &[GatewayTool],
        encode: impl Fn(&str) -> String,
    ) -> anyhow::Result<Self> {
        let mut canonical_to_wire = BTreeMap::new();
        let mut wire_to_canonical = BTreeMap::new();

        for tool in tools {
            if canonical_to_wire.contains_key(&tool.id) {
                anyhow::bail!("duplicate canonical tool id");
            }
            let wire = encode(&tool.id);
            validate_wire_name(&wire)?;
            if wire_to_canonical.contains_key(&wire) {
                anyhow::bail!("provider tool name collision");
            }
            canonical_to_wire.insert(tool.id.clone(), wire.clone());
            wire_to_canonical.insert(wire, tool.id.clone());
        }

        Ok(Self {
            canonical_to_wire,
            wire_to_canonical,
        })
    }

    pub fn wire_name(&self, canonical: &str) -> Option<&str> {
        self.canonical_to_wire.get(canonical).map(String::as_str)
    }

    pub fn canonical_name(&self, wire: &str) -> Option<&str> {
        self.wire_to_canonical.get(wire).map(String::as_str)
    }

    #[cfg(test)]
    pub(crate) fn from_tools_with_test_encoder(
        tools: &[GatewayTool],
        encode: impl Fn(&str) -> String,
    ) -> anyhow::Result<Self> {
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

fn validate_wire_name(wire: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !wire.is_empty()
            && wire.len() <= MAX_WIRE_NAME_BYTES
            && wire
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_'),
        "invalid provider tool name"
    );
    Ok(())
}
