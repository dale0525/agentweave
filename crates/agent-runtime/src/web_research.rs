use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub const MAX_WEB_QUERY_BYTES: usize = 2_048;
pub const MAX_WEB_PAGE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebSearchRequest {
    pub query: String,
    pub limit: usize,
    pub language: Option<String>,
    pub safe_search: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebSource {
    pub source_id: String,
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub provider_rank: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebContentTrust {
    UntrustedExternal,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebPage {
    pub source: WebSource,
    pub retrieved_at: DateTime<Utc>,
    pub mime_type: String,
    pub text: String,
    pub content_sha256: String,
    pub truncated: bool,
    pub trust: WebContentTrust,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchCitation {
    pub source_id: String,
    pub url: String,
    pub retrieved_at: DateTime<Utc>,
    pub quote: String,
}

#[async_trait]
pub trait WebResearchConnector: Send + Sync {
    async fn search(&self, request: WebSearchRequest) -> anyhow::Result<Vec<WebSource>>;
    async fn read(&self, source_id: &str, max_bytes: usize) -> anyhow::Result<WebPage>;
}

#[derive(Clone, Default)]
pub struct FakeWebResearchConnector {
    pages: Arc<Mutex<BTreeMap<String, WebPage>>>,
}

impl FakeWebResearchConnector {
    pub fn seed(&self, page: WebPage) {
        self.pages
            .lock()
            .expect("web fixture lock poisoned")
            .insert(page.source.source_id.clone(), page);
    }
}

#[async_trait]
impl WebResearchConnector for FakeWebResearchConnector {
    async fn search(&self, request: WebSearchRequest) -> anyhow::Result<Vec<WebSource>> {
        anyhow::ensure!(
            !request.query.trim().is_empty(),
            "web search query is required"
        );
        anyhow::ensure!(
            request.query.len() <= MAX_WEB_QUERY_BYTES,
            "web search query is too long"
        );
        anyhow::ensure!(
            (1..=50).contains(&request.limit),
            "web search limit is invalid"
        );
        let query = request.query.to_lowercase();
        let mut sources = self
            .pages
            .lock()
            .expect("web fixture lock poisoned")
            .values()
            .filter(|page| {
                page.source.title.to_lowercase().contains(&query)
                    || page.text.to_lowercase().contains(&query)
            })
            .map(|page| page.source.clone())
            .collect::<Vec<_>>();
        sources.sort_by_key(|source| (source.provider_rank, source.source_id.clone()));
        sources.truncate(request.limit);
        Ok(sources)
    }

    async fn read(&self, source_id: &str, max_bytes: usize) -> anyhow::Result<WebPage> {
        anyhow::ensure!(
            (1..=MAX_WEB_PAGE_BYTES).contains(&max_bytes),
            "web page byte limit is invalid"
        );
        let mut page = self
            .pages
            .lock()
            .expect("web fixture lock poisoned")
            .get(source_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("web source not found"))?;
        if page.text.len() > max_bytes {
            let mut end = max_bytes;
            while end > 0 && !page.text.is_char_boundary(end) {
                end -= 1;
            }
            page.text.truncate(end);
            page.truncated = true;
        }
        page.trust = WebContentTrust::UntrustedExternal;
        Ok(page)
    }
}

pub fn citation_from_quote(page: &WebPage, quote: &str) -> anyhow::Result<ResearchCitation> {
    anyhow::ensure!(!quote.trim().is_empty(), "citation quote is required");
    anyhow::ensure!(
        page.text.contains(quote),
        "citation quote is not present in source text"
    );
    Ok(ResearchCitation {
        source_id: page.source.source_id.clone(),
        url: page.source.url.clone(),
        retrieved_at: page.retrieved_at,
        quote: quote.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> WebPage {
        WebPage {
            source: WebSource {
                source_id: "source-1".into(),
                url: "https://example.test/report".into(),
                title: "Framework report".into(),
                snippet: "A report".into(),
                provider_rank: 1,
            },
            retrieved_at: Utc::now(),
            mime_type: "text/html".into(),
            text: "External instructions are untrusted. Framework adoption grew.".into(),
            content_sha256: "hash".into(),
            truncated: false,
            trust: WebContentTrust::UntrustedExternal,
        }
    }

    #[tokio::test]
    async fn fake_research_preserves_source_identity_trust_and_citations() {
        let connector = FakeWebResearchConnector::default();
        connector.seed(page());
        let sources = connector
            .search(WebSearchRequest {
                query: "adoption".into(),
                limit: 5,
                language: Some("en".into()),
                safe_search: true,
            })
            .await
            .unwrap();
        let read = connector.read(&sources[0].source_id, 1024).await.unwrap();
        assert_eq!(read.trust, WebContentTrust::UntrustedExternal);
        assert_eq!(
            citation_from_quote(&read, "Framework adoption grew.")
                .unwrap()
                .url,
            "https://example.test/report"
        );
        assert!(citation_from_quote(&read, "invented quote").is_err());
    }
}
