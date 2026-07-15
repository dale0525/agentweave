use agent_runtime::credential::SecretMaterial;
use agent_runtime::oauth::{
    OAuthAuthorizationPlan, OAuthAuthorizationUrlRequest, OAuthCodeExchangeRequest, OAuthProvider,
    OAuthProviderError, OAuthProviderErrorCode, OAuthRefreshRequest, OAuthTokenGrant,
};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use reqwest::Url;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration as StdDuration;

pub const GOOGLE_PROVIDER_ID: &str = "google-workspace";

pub struct WorkspaceOAuthProvider {
    provider_id: String,
    authorization_origin: String,
    authorization_endpoint: Url,
    token_endpoint: Url,
    userinfo_endpoint: Url,
    client_id: String,
    client_secret: Option<SecretMaterial>,
    connector_scopes: BTreeMap<String, BTreeSet<String>>,
    capability_scopes: BTreeMap<String, BTreeSet<String>>,
    authorization_parameters: BTreeMap<String, String>,
    client: reqwest::Client,
}

impl WorkspaceOAuthProvider {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider_id: impl Into<String>,
        authorization_endpoint: &str,
        token_endpoint: &str,
        userinfo_endpoint: &str,
        client_id: impl Into<String>,
        client_secret: Option<SecretMaterial>,
        connector_scopes: BTreeMap<String, BTreeSet<String>>,
        capability_scopes: BTreeMap<String, BTreeSet<String>>,
        authorization_parameters: BTreeMap<String, String>,
    ) -> anyhow::Result<Self> {
        let provider_id = provider_id.into();
        let client_id = client_id.into();
        anyhow::ensure!(
            !provider_id.trim().is_empty() && provider_id.len() <= 64,
            "OAuth provider id is invalid"
        );
        anyhow::ensure!(
            !client_id.trim().is_empty() && client_id.len() <= 2048,
            "OAuth client id is invalid"
        );
        anyhow::ensure!(
            !connector_scopes.is_empty(),
            "OAuth connector scopes are required"
        );
        let authorization_endpoint = secure_url(authorization_endpoint)?;
        let token_endpoint = secure_url(token_endpoint)?;
        let userinfo_endpoint = secure_url(userinfo_endpoint)?;
        let authorization_origin = authorization_endpoint.origin().ascii_serialization();
        Ok(Self {
            provider_id,
            authorization_origin,
            authorization_endpoint,
            token_endpoint,
            userinfo_endpoint,
            client_id,
            client_secret,
            connector_scopes,
            capability_scopes,
            authorization_parameters,
            client: reqwest::Client::builder()
                .timeout(StdDuration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }

    pub fn google(
        client_id: impl Into<String>,
        client_secret: Option<SecretMaterial>,
    ) -> anyhow::Result<Self> {
        let connector_scopes = BTreeMap::from([
            (
                agent_runtime::mail_connector_transport::MAIL_CONNECTOR_ID.into(),
                scopes(&[
                    "https://www.googleapis.com/auth/gmail.modify",
                    "https://www.googleapis.com/auth/gmail.compose",
                    "https://www.googleapis.com/auth/gmail.send",
                ]),
            ),
            (
                agent_runtime::calendar_connector_transport::CALENDAR_CONNECTOR_ID.into(),
                scopes(&["https://www.googleapis.com/auth/calendar.events"]),
            ),
            (
                agent_runtime::contacts_connector_transport::CONTACTS_CONNECTOR_ID.into(),
                scopes(&[
                    "https://www.googleapis.com/auth/contacts",
                    "https://www.googleapis.com/auth/contacts.readonly",
                ]),
            ),
        ]);
        let capability_scopes = BTreeMap::from([
            (
                "mail".into(),
                connector_scopes[agent_runtime::mail_connector_transport::MAIL_CONNECTOR_ID]
                    .clone(),
            ),
            (
                "calendar".into(),
                connector_scopes
                    [agent_runtime::calendar_connector_transport::CALENDAR_CONNECTOR_ID]
                    .clone(),
            ),
            (
                "contacts".into(),
                connector_scopes
                    [agent_runtime::contacts_connector_transport::CONTACTS_CONNECTOR_ID]
                    .clone(),
            ),
        ]);
        Self::new(
            GOOGLE_PROVIDER_ID,
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            "https://openidconnect.googleapis.com/v1/userinfo",
            client_id,
            client_secret,
            connector_scopes,
            capability_scopes,
            BTreeMap::from([
                ("access_type".into(), "offline".into()),
                ("include_granted_scopes".into(), "true".into()),
                ("prompt".into(), "consent".into()),
            ]),
        )
    }

    async fn token_request(
        &self,
        mut form: Vec<(String, String)>,
        expected_scopes: Option<&BTreeSet<String>>,
    ) -> Result<OAuthTokenGrant, OAuthProviderError> {
        form.push(("client_id".into(), self.client_id.clone()));
        if let Some(secret) = &self.client_secret {
            let value = secret
                .with_exposed_bytes(|bytes| std::str::from_utf8(bytes).map(str::to_owned))
                .map_err(|_| OAuthProviderError::new(OAuthProviderErrorCode::InvalidRequest))?;
            form.push(("client_secret".into(), value));
        }
        let response = self
            .client
            .post(self.token_endpoint.clone())
            .form(&form)
            .send()
            .await
            .map_err(|_| OAuthProviderError::new(OAuthProviderErrorCode::Unavailable))?;
        if !response.status().is_success() {
            return Err(OAuthProviderError::new(
                OAuthProviderErrorCode::ExchangeFailed,
            ));
        }
        let token: TokenResponse = response
            .json()
            .await
            .map_err(|_| OAuthProviderError::new(OAuthProviderErrorCode::ExchangeFailed))?;
        let granted_scopes = token
            .scope
            .as_deref()
            .map(|value| value.split_whitespace().map(str::to_string).collect())
            .or_else(|| expected_scopes.cloned())
            .unwrap_or_default();
        if let Some(expected) = expected_scopes
            && !expected.is_subset(&granted_scopes)
        {
            return Err(OAuthProviderError::new(
                OAuthProviderErrorCode::PermissionInsufficient,
            ));
        }
        let access_token = SecretMaterial::new(token.access_token)
            .map_err(|_| OAuthProviderError::new(OAuthProviderErrorCode::ExchangeFailed))?;
        let subject = self.user_subject(&access_token).await?;
        Ok(OAuthTokenGrant {
            provider_subject: subject,
            access_token,
            refresh_token: token
                .refresh_token
                .map(SecretMaterial::new)
                .transpose()
                .map_err(|_| OAuthProviderError::new(OAuthProviderErrorCode::ExchangeFailed))?,
            granted_scopes,
            expires_at: token
                .expires_in
                .map(|seconds| Utc::now() + Duration::seconds(seconds.max(1))),
        })
    }

    async fn user_subject(
        &self,
        access_token: &SecretMaterial,
    ) -> Result<String, OAuthProviderError> {
        let token = access_token
            .with_exposed_bytes(|bytes| std::str::from_utf8(bytes).map(str::to_owned))
            .map_err(|_| OAuthProviderError::new(OAuthProviderErrorCode::ExchangeFailed))?;
        let response = self
            .client
            .get(self.userinfo_endpoint.clone())
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| OAuthProviderError::new(OAuthProviderErrorCode::Unavailable))?;
        let user: UserInfo = response
            .json()
            .await
            .map_err(|_| OAuthProviderError::new(OAuthProviderErrorCode::ExchangeFailed))?;
        if user.sub.trim().is_empty() || user.sub.len() > 255 {
            return Err(OAuthProviderError::new(
                OAuthProviderErrorCode::ExchangeFailed,
            ));
        }
        Ok(user.sub)
    }
}

#[async_trait]
impl OAuthProvider for WorkspaceOAuthProvider {
    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn authorization_origin(&self) -> &str {
        &self.authorization_origin
    }

    fn authorization_plan(
        &self,
        connector_ids: &BTreeSet<String>,
        capabilities: &BTreeSet<String>,
    ) -> Result<OAuthAuthorizationPlan, OAuthProviderError> {
        let mut requested_scopes = BTreeSet::new();
        let mut connector_scopes = BTreeMap::new();
        for connector_id in connector_ids {
            let scopes = self
                .connector_scopes
                .get(connector_id)
                .ok_or_else(|| OAuthProviderError::new(OAuthProviderErrorCode::InvalidRequest))?;
            requested_scopes.extend(scopes.iter().cloned());
            connector_scopes.insert(connector_id.clone(), scopes.clone());
        }
        for capability in capabilities {
            let scopes = self
                .capability_scopes
                .get(capability)
                .ok_or_else(|| OAuthProviderError::new(OAuthProviderErrorCode::InvalidRequest))?;
            requested_scopes.extend(scopes.iter().cloned());
        }
        Ok(OAuthAuthorizationPlan {
            requested_scopes,
            connector_scopes,
        })
    }

    fn authorization_url(
        &self,
        request: OAuthAuthorizationUrlRequest,
    ) -> Result<String, OAuthProviderError> {
        let mut url = self.authorization_endpoint.clone();
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("client_id", &self.client_id)
                .append_pair("redirect_uri", &request.redirect_uri)
                .append_pair("response_type", "code")
                .append_pair("state", &request.state)
                .append_pair("code_challenge", &request.pkce_challenge)
                .append_pair("code_challenge_method", "S256")
                .append_pair(
                    "scope",
                    &request.scopes.iter().cloned().collect::<Vec<_>>().join(" "),
                );
            for (name, value) in &self.authorization_parameters {
                query.append_pair(name, value);
            }
        }
        Ok(url.to_string())
    }

    async fn exchange_code(
        &self,
        request: OAuthCodeExchangeRequest,
    ) -> Result<OAuthTokenGrant, OAuthProviderError> {
        self.token_request(
            vec![
                ("grant_type".into(), "authorization_code".into()),
                ("code".into(), request.code.expose().into()),
                ("redirect_uri".into(), request.redirect_uri),
                (
                    "code_verifier".into(),
                    request.pkce_verifier.expose().into(),
                ),
            ],
            Some(&request.requested_scopes),
        )
        .await
    }

    async fn refresh_token(
        &self,
        request: OAuthRefreshRequest,
    ) -> Result<OAuthTokenGrant, OAuthProviderError> {
        self.token_request(
            vec![
                ("grant_type".into(), "refresh_token".into()),
                (
                    "refresh_token".into(),
                    request.refresh_token.expose().into(),
                ),
            ],
            None,
        )
        .await
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    scope: Option<String>,
    expires_in: Option<i64>,
}

#[derive(Deserialize)]
struct UserInfo {
    sub: String,
}

fn scopes(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn secure_url(value: &str) -> anyhow::Result<Url> {
    let url = Url::parse(value)?;
    anyhow::ensure!(url.scheme() == "https", "OAuth endpoint must use HTTPS");
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "OAuth endpoint cannot contain credentials"
    );
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_plan_and_url_bind_pkce_and_connector_scopes() {
        let provider = WorkspaceOAuthProvider::google("client", None).unwrap();
        let connectors = BTreeSet::from([
            agent_runtime::calendar_connector_transport::CALENDAR_CONNECTOR_ID.into(),
            agent_runtime::contacts_connector_transport::CONTACTS_CONNECTOR_ID.into(),
        ]);
        let plan = provider
            .authorization_plan(&connectors, &BTreeSet::new())
            .unwrap();
        assert_eq!(plan.connector_scopes.len(), 2);
        let url = provider
            .authorization_url(OAuthAuthorizationUrlRequest {
                authorization_id: "authorization-1".into(),
                redirect_uri: "http://127.0.0.1/callback".into(),
                state: "state".into(),
                pkce_challenge: "challenge".into(),
                scopes: plan.requested_scopes,
            })
            .unwrap();
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("access_type=offline"));
    }
}
