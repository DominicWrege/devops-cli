// Adapted from SNcom-GmbH/az-mise-plugin, commit 60959ab1b7ff637265d13168909a34655600f6fb.
// Copyright (c) 2026 SNcom GmbH. MIT license, see LICENSES/az-mise-plugin.txt.
use crate::{
    config::{Settings, Store},
    error::{Failure, auth_required},
};
use anyhow::{Result, ensure};
use azure_core::credentials::AccessToken;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use windows::{
    Security::Authentication::Web::Core::{
        WebAuthenticationCoreManager, WebTokenRequest, WebTokenRequestResult, WebTokenRequestStatus,
    },
    Win32::{
        System::{
            Console::GetConsoleWindow,
            WinRT::{
                IWebAuthenticationCoreManagerInterop, RO_INIT_MULTITHREADED, RoInitialize,
                RoUninitialize,
            },
        },
        UI::WindowsAndMessaging::GetForegroundWindow,
    },
    core::HSTRING,
};

#[derive(Serialize, Deserialize)]
struct Account {
    tenant_id: String,
    client_id: String,
    account_id: String,
}

pub async fn acquire(store: Store, settings: Settings, interactive: bool) -> Result<AccessToken> {
    tokio::task::spawn_blocking(move || match on_thread(&store, &settings, false) {
        Ok(token) => Ok(token),
        Err(error)
            if interactive && error.downcast_ref::<Failure>().is_some_and(|e| e.code == 3) =>
        {
            on_thread(&store, &settings, true)
        }
        Err(error) => Err(error),
    })
    .await?
}

fn on_thread(store: &Store, settings: &Settings, interactive: bool) -> Result<AccessToken> {
    let (tenant, client) = settings.identity()?;
    // SAFETY: This blocking thread initializes its own MTA. A guard is created
    // only after success and is dropped after all COM objects on this thread.
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(windows_error)?;
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            // SAFETY: Balances this thread's successful RoInitialize, including S_FALSE.
            unsafe { RoUninitialize() };
        }
    }
    let _apartment = Apartment;
    let provider = WebAuthenticationCoreManager::FindAccountProviderWithAuthorityAsync(
        &HSTRING::from("https://login.microsoft.com"),
        &HSTRING::from("organizations"),
    )
    .map_err(windows_error)?
    .join()
    .map_err(windows_error)?;
    let request = WebTokenRequest::Create(&provider, &HSTRING::new(), &HSTRING::from(&client))
        .map_err(windows_error)?;
    let properties = request.Properties().map_err(windows_error)?;
    properties
        .Insert(&HSTRING::from("resource"), &HSTRING::from(super::RESOURCE))
        .map_err(windows_error)?;
    properties
        .Insert(
            &HSTRING::from("authority"),
            &HSTRING::from(format!("https://login.microsoftonline.com/{tenant}")),
        )
        .map_err(windows_error)?;
    let saved: Option<Account> = store.read("wam-account.json")?;
    let saved = saved.filter(|a| a.tenant_id == tenant && a.client_id == client);
    let result = if interactive {
        // SAFETY: These OS queries have no pointer arguments or caller preconditions.
        let mut window = unsafe { GetConsoleWindow() };
        if window.is_invalid() {
            // SAFETY: As above. A null handle is checked before use.
            window = unsafe { GetForegroundWindow() };
        }
        ensure!(
            !window.is_invalid(),
            "WAM sign-in needs an interactive Windows desktop"
        );
        let interop: IWebAuthenticationCoreManagerInterop =
            windows::core::factory::<WebAuthenticationCoreManager, _>().map_err(windows_error)?;
        // SAFETY: The non-null HWND comes from Windows and the request and interop
        // objects stay alive through join. This runs only during interactive setup.
        let operation: windows_future::IAsyncOperation<WebTokenRequestResult> =
            unsafe { interop.RequestTokenForWindowAsync(window, &request) }
                .map_err(windows_error)?;
        operation.join().map_err(windows_error)?
    } else if let Some(account) = &saved {
        let account = WebAuthenticationCoreManager::FindAccountAsync(
            &provider,
            &HSTRING::from(&account.account_id),
        )
        .map_err(windows_error)?
        .join()
        .map_err(|_| auth_required())?;
        WebAuthenticationCoreManager::GetTokenSilentlyWithWebAccountAsync(&request, &account)
            .map_err(windows_error)?
            .join()
            .map_err(windows_error)?
    } else {
        WebAuthenticationCoreManager::GetTokenSilentlyAsync(&request)
            .map_err(windows_error)?
            .join()
            .map_err(windows_error)?
    };
    let status = result.ResponseStatus().map_err(windows_error)?;
    if status == WebTokenRequestStatus::UserInteractionRequired
        || status == WebTokenRequestStatus::AccountSwitch
    {
        return Err(auth_required());
    }
    ensure!(
        status != WebTokenRequestStatus::UserCancel,
        "WAM sign-in canceled"
    );
    ensure!(
        status == WebTokenRequestStatus::Success,
        "WAM sign-in failed with status {}. Check app registration and tenant policy.",
        status.0
    );
    let response = result
        .ResponseData()
        .map_err(windows_error)?
        .GetAt(0)
        .map_err(windows_error)?;
    let token = response.Token().map_err(windows_error)?.to_string();
    ensure!(!token.is_empty(), "WAM returned no token");
    let account_id = response
        .WebAccount()
        .map_err(windows_error)?
        .Id()
        .map_err(windows_error)?
        .to_string();
    store.write(
        "wam-account.json",
        &Account {
            tenant_id: tenant,
            client_id: client,
            account_id,
        },
    )?;
    // Decode only to schedule renewal. Server-side validation still authorizes requests.
    let expiry = token
        .split('.')
        .nth(1)
        .and_then(|p| URL_SAFE_NO_PAD.decode(p).ok())
        .and_then(|p| serde_json::from_slice::<serde_json::Value>(&p).ok())
        .and_then(|p| p["exp"].as_i64())
        .and_then(|exp| OffsetDateTime::from_unix_timestamp(exp).ok())
        .unwrap_or_else(|| OffsetDateTime::now_utc() + Duration::minutes(10));
    Ok(AccessToken::new(token, expiry))
}

fn windows_error(error: windows::core::Error) -> anyhow::Error {
    anyhow::anyhow!(
        "WAM API failed, HRESULT 0x{:08X}. Check broker availability and app registration.",
        error.code().0 as u32
    )
}
