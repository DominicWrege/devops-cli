use crate::config::{Settings, Store};
use anyhow::Result;
use azure_core::credentials::{AccessToken, TokenCredential, TokenRequestOptions};
use std::sync::Arc;

#[cfg(not(windows))]
mod device;
#[cfg(windows)]
#[allow(unsafe_code)] // Native WAM interop is the only audited FFI boundary.
mod wam;

#[cfg(windows)]
pub const RESOURCE: &str = "499b84ac-1321-427f-aa17-267ca6975798";
pub const SCOPE: &str = "499b84ac-1321-427f-aa17-267ca6975798/.default";

pub struct Credential {
    store: Store,
    settings: Settings,
    #[cfg(windows)]
    cached: tokio::sync::Mutex<Option<AccessToken>>,
}
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DevOpsCredential { .. }")
    }
}
impl Credential {
    pub fn new(store: Store, settings: Settings) -> Result<Arc<Self>> {
        settings.identity()?;
        Ok(Arc::new(Self {
            store,
            settings,
            #[cfg(windows)]
            cached: tokio::sync::Mutex::new(None),
        }))
    }
    pub async fn token(&self) -> Result<AccessToken> {
        #[cfg(windows)]
        {
            let mut cached = self.cached.lock().await;
            if let Some(token) = cached.as_ref().filter(|t| {
                t.expires_on > time::OffsetDateTime::now_utc() + time::Duration::minutes(5)
            }) {
                return Ok(token.clone());
            }
            let token = wam::acquire(self.store.clone(), self.settings.clone(), false).await?;
            *cached = Some(token.clone());
            Ok(token)
        }
        #[cfg(not(windows))]
        {
            device::silent(&self.store, &self.settings).await
        }
    }
}
#[async_trait::async_trait]
impl TokenCredential for Credential {
    async fn get_token(
        &self,
        scopes: &[&str],
        _: Option<TokenRequestOptions<'_>>,
    ) -> azure_core::Result<AccessToken> {
        if scopes != [SCOPE] {
            return Err(azure_core::Error::with_message(
                azure_core::error::ErrorKind::Credential,
                "Unsupported token audience",
            ));
        }
        self.token().await.map_err(|_| {
            azure_core::Error::with_message(
                azure_core::error::ErrorKind::Credential,
                "DEVOPS_AUTH_REQUIRED: Sign-in required. Open devops tui.",
            )
        })
    }
}

pub fn summary(settings: &Settings, token: &AccessToken) -> Result<String> {
    let (tenant, client) = settings.identity()?;
    Ok(format!(
        "Signed in\nTenant ID: {tenant}\nClient ID: {client}\nOrganization: {}\nProject: {}\nToken expires: {}",
        settings.organization.as_deref().unwrap_or("(not set)"),
        settings.project.as_deref().unwrap_or("(not set)"),
        token
            .expires_on
            .format(&time::format_description::well_known::Rfc3339)?,
    ))
}

pub async fn discover_tenant(organization: &str) -> Result<String> {
    discover_tenant_at("https://dev.azure.com", organization).await
}

async fn discover_tenant_at(base: &str, organization: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let url = format!("{base}/{organization}/_apis/connectionData?api-version=7.1-preview.1");
    let response = client.get(url).send().await?;
    let headers = response.headers();
    if let Some(tenant) = headers
        .get("x-vss-resourcetenant")
        .and_then(|value| value.to_str().ok())
    {
        return tenant_id(tenant);
    }
    for value in headers.get_all("www-authenticate") {
        if let Some(tenant) = value
            .to_str()
            .ok()
            .and_then(|value| value.split("authorization_uri=").nth(1))
            .and_then(|value| value.split([',', ' ']).next())
            .and_then(|value| value.rsplit('/').next())
        {
            return tenant_id(tenant);
        }
    }
    anyhow::bail!(
        "Cannot discover the tenant for organization {organization} (HTTP {}). Pass --tenant-id.",
        response.status().as_u16()
    )
}

fn tenant_id(value: &str) -> Result<String> {
    let tenant = uuid::Uuid::parse_str(value.trim())
        .map_err(|_| anyhow::anyhow!("Azure DevOps returned an invalid tenant ID"))?;
    Ok(tenant.to_string())
}

pub async fn login(
    store: &Store,
    settings: &Settings,
    notify: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<AccessToken> {
    #[cfg(windows)]
    {
        let _ = notify;
        wam::acquire(store.clone(), settings.clone(), true).await
    }
    #[cfg(not(windows))]
    {
        device::login(store, settings, notify).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    #[tokio::test]
    async fn discovers_the_tenant_from_the_organization_endpoint() -> Result<()> {
        let server = wiremock::MockServer::start().await;
        let tenant = "33333333-3333-3333-3333-333333333333";
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/org/_apis/connectionData"))
            .respond_with(
                wiremock::ResponseTemplate::new(302).insert_header("x-vss-resourcetenant", tenant),
            )
            .mount(&server)
            .await;
        assert_eq!(discover_tenant_at(&server.uri(), "org").await?, tenant);
        Ok(())
    }

    #[tokio::test]
    async fn discovers_the_tenant_from_the_authentication_challenge() -> Result<()> {
        let server = wiremock::MockServer::start().await;
        let tenant = "44444444-4444-4444-4444-444444444444";
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/org/_apis/connectionData"))
            .respond_with(wiremock::ResponseTemplate::new(401).insert_header(
                "www-authenticate",
                format!("Bearer authorization_uri=https://login.microsoftonline.com/{tenant}"),
            ))
            .mount(&server)
            .await;
        assert_eq!(discover_tenant_at(&server.uri(), "org").await?, tenant);
        Ok(())
    }

    #[test]
    fn summary_reports_configuration_and_expiry_without_the_access_token() -> Result<()> {
        let settings = Settings {
            organization: Some("org".into()),
            project: Some("project".into()),
            tenant_id: Some("11111111-1111-1111-1111-111111111111".into()),
            client_id: Some("22222222-2222-2222-2222-222222222222".into()),
            ..Default::default()
        };
        let expiry = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
        let token = AccessToken::new("access-secret", expiry);
        let text = summary(&settings, &token)?;
        assert!(text.contains("Tenant ID: 11111111-1111-1111-1111-111111111111"));
        assert!(text.contains("Client ID: 22222222-2222-2222-2222-222222222222"));
        assert!(text.contains("Organization: org"));
        assert!(text.contains("Project: project"));
        assert!(
            text.contains(
                expiry
                    .format(&time::format_description::well_known::Rfc3339)?
                    .as_str()
            )
        );
        assert!(!text.contains("access-secret"));
        Ok(())
    }
}
