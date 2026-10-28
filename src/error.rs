use std::fmt;

#[derive(Debug)]
pub struct Failure {
    pub code: u8,
    pub message: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Failure {}

#[derive(Debug)]
pub struct HttpFailure(pub u16);
impl fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Azure DevOps request failed, HTTP {}. No automatic write retry was attempted.",
            self.0
        )
    }
}
impl std::error::Error for HttpFailure {}

pub fn invalid(message: impl Into<String>) -> anyhow::Error {
    Failure {
        code: 2,
        message: message.into(),
    }
    .into()
}

pub fn auth_required() -> anyhow::Error {
    Failure {
        code: 3,
        message: "Sign-in required. Run `devops login` or open `devops tui` to enter a new token. For PAT authentication, check token expiry, organization access, and DEVOPS_PAT if set.".into(),
    }
    .into()
}

pub fn sdk(error: azure_core::Error) -> anyhow::Error {
    // Never display response bodies, headers, or credential error chains.
    let status = error.http_status().map(u16::from);
    if status.is_none() && error.to_string().contains("DEVOPS_AUTH_REQUIRED") {
        return auth_required();
    }
    match status {
        Some(401) => auth_required(),
        Some(403) => Failure {
            code: 4,
            message: "Azure DevOps denied the operation. Check permissions and policies.".into(),
        }
        .into(),
        Some(status) => HttpFailure(status).into(),
        None => anyhow::anyhow!(
            "Azure DevOps request failed. Check connectivity and sign in through `devops tui` if needed."
        ),
    }
}
