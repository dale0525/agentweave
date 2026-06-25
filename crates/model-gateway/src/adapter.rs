use crate::provider::ProviderProfile;
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use serde_json::Value;
use std::pin::Pin;

pub type GatewayStream = Pin<Box<dyn Stream<Item = anyhow::Result<Bytes>> + Send>>;

#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports(&self, profile: &ProviderProfile) -> bool;
    async fn stream(
        &self,
        profile: &ProviderProfile,
        request: Value,
    ) -> anyhow::Result<GatewayStream>;
}
