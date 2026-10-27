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

pub async fn login(
    store: &Store,
    settings: &Settings,
    notify: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<()> {
    #[cfg(windows)]
    {
        let _ = notify;
        wam::acquire(store.clone(), settings.clone(), true).await?;
    }
    #[cfg(not(windows))]
    {
        device::login(store, settings, notify).await?;
    }
    Ok(())
}
