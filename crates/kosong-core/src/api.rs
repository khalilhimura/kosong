//! HTTP client for the remote API.
//!
//! Mirrors the endpoints in technical specification §8.2 and §9.2, and
//! implements the client-handling column of the error table in §11:
//!
//! | HTTP | Client behaviour |
//! |---|---|
//! | 400 | Show the repair guidance; do not retry |
//! | 401 | Attempt one safe refresh, then require login |
//! | 409 | Preserve both versions and ask the user |
//! | 429 | Show retry guidance; do not hammer the endpoint |
//!
//! The one-refresh rule matters: retrying a refresh in a loop against a
//! revoked token family is exactly the pattern that looks like an attack.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Default API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.kosong.thefutureissolo.com";

/// Per-request timeout. A beginner should not watch a terminal hang.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("could not reach the kosong service")]
    Network(#[source] reqwest::Error),

    #[error("the service took too long to answer")]
    Timeout,

    #[error("you are not signed in")]
    Unauthenticated,

    #[error("your session has expired")]
    SessionExpired,

    #[error("you have not synced a page yet")]
    NotFound,

    /// The remote changed since the version this write was based on.
    #[error("the page on the server changed since your last sync")]
    Conflict(Box<ConflictDetails>),

    #[error("too many requests; try again in {retry_after_seconds} seconds")]
    RateLimited { retry_after_seconds: u64 },

    #[error("{message}")]
    Rejected { code: String, message: String },

    /// The service is running but cannot do this right now, and said why.
    ///
    /// Distinct from [`Server`](Self::Server): that is a fault the user can do
    /// nothing about except report, whereas this carries a sentence written
    /// for them — the email provider being unreachable is the case in hand.
    #[error("{message}")]
    Unavailable { message: String },

    #[error("the service had a problem (request {request_id})")]
    Server { request_id: String },

    #[error("could not understand the service's answer")]
    Malformed,
}

impl ApiError {
    /// A plain-language next action, in the CLI's voice.
    pub fn repair(&self) -> String {
        match self {
            Self::Network(_) => {
                "Check your internet connection and try again. Everything local still works \
                 without a network."
                    .into()
            }
            Self::Timeout => "Try again in a moment.".into(),
            Self::Unauthenticated | Self::SessionExpired => "Run: kosong login".into(),
            Self::NotFound => "Run `kosong sync --push` to put your page on the server.".into(),
            Self::Conflict(_) => {
                "Both versions have been saved so nothing is lost. Run `kosong sync` to see them."
                    .into()
            }
            Self::RateLimited {
                retry_after_seconds,
            } => {
                format!("Wait {retry_after_seconds} seconds, then try again.")
            }
            Self::Rejected { .. } => "Fix the problem above, then try again.".into(),
            Self::Unavailable { .. } => {
                "Try again in a few minutes. Everything local still works without the service."
                    .into()
            }
            Self::Server { request_id } => {
                format!("This is a problem on our side. If you report it, quote: {request_id}")
            }
            // Not "run kosong update": no such command exists, and sending
            // someone to one is how a repair action stops being trusted.
            Self::Malformed => "Install the current version of kosong:\n  \
                 curl -fsSL https://kosong.thefutureissolo.com/install.sh | sh"
                .into(),
        }
    }

    /// Whether a single token refresh might fix this.
    pub fn is_expired_session(&self) -> bool {
        matches!(self, Self::SessionExpired)
    }
}

/// What the server said about a conflicting write.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConflictDetails {
    pub remote_etag: Option<String>,
    pub remote_updated_at: Option<String>,
    pub remote_byte_size: Option<u64>,
}

/// A signed-in session.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserSummary {
    pub id: String,
    /// Masked address, e.g. `k***@example.com`. Never the full address.
    pub email_hint: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SignInResult {
    #[serde(flatten)]
    pub tokens: TokenPair,
    pub user: UserSummary,
}

/// A document as stored remotely.
#[derive(Debug, Clone)]
pub struct RemoteDocument {
    pub etag: String,
    pub updated_at: String,
    /// Raw OKF bytes, already decoded.
    pub bytes: Vec<u8>,
}

/// Result of a successful write.
#[derive(Debug, Clone, Deserialize)]
pub struct WriteResult {
    pub etag: String,
    pub updated_at: String,
}

// Wire shapes, kept private so the public API never leaks transport details.

#[derive(Debug, Deserialize)]
struct ErrorBody {
    code: String,
    message: String,
    request_id: Option<String>,
    #[serde(flatten)]
    conflict: ConflictDetails,
}

#[derive(Debug, Deserialize)]
struct DocumentBody {
    etag: String,
    updated_at: String,
    document: String,
}

#[derive(Debug, Serialize)]
struct CodeRequestBody<'a> {
    email: &'a str,
}

#[derive(Debug, Serialize)]
struct CodeVerifyBody<'a> {
    email: &'a str,
    code: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_name: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct RefreshBody<'a> {
    refresh_token: &'a str,
}

#[derive(Debug, Serialize)]
struct DocumentWriteBody {
    document: String,
}

/// Client for the kosong remote API.
#[derive(Debug, Clone)]
pub struct ApiClient {
    base_url: String,
    http: reqwest::Client,
}

impl ApiClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("kosong/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(ApiError::Network)?;

        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            http,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    // -- Authentication -----------------------------------------------------

    /// Asks the server to email a verification code.
    pub async fn request_code(&self, email: &str) -> Result<(), ApiError> {
        let response = self
            .http
            .post(self.url("/v1/auth/code/request"))
            .json(&CodeRequestBody { email })
            .send()
            .await
            .map_err(classify_transport)?;

        check_status(response).await?;
        Ok(())
    }

    /// Exchanges a code for a session.
    pub async fn verify_code(
        &self,
        email: &str,
        code: &str,
        device_name: Option<&str>,
    ) -> Result<SignInResult, ApiError> {
        let response = self
            .http
            .post(self.url("/v1/auth/code/verify"))
            .json(&CodeVerifyBody {
                email,
                code,
                device_name,
            })
            .send()
            .await
            .map_err(classify_transport)?;

        let response = check_status(response).await?;
        response.json().await.map_err(|_| ApiError::Malformed)
    }

    /// Rotates the refresh token, returning a new pair.
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, ApiError> {
        let response = self
            .http
            .post(self.url("/v1/auth/refresh"))
            .json(&RefreshBody { refresh_token })
            .send()
            .await
            .map_err(classify_transport)?;

        let response = check_status(response).await?;
        response.json().await.map_err(|_| ApiError::Malformed)
    }

    /// Revokes a refresh token server-side.
    pub async fn logout(&self, refresh_token: &str) -> Result<(), ApiError> {
        let response = self
            .http
            .post(self.url("/v1/auth/logout"))
            .json(&RefreshBody { refresh_token })
            .send()
            .await
            .map_err(classify_transport)?;

        check_status(response).await?;
        Ok(())
    }

    /// Deletes the account and everything stored for it.
    pub async fn delete_account(&self, access_token: &str) -> Result<(), ApiError> {
        let response = self
            .http
            .delete(self.url("/v1/account"))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(classify_transport)?;

        check_status(response).await?;
        Ok(())
    }

    // -- Documents ----------------------------------------------------------

    /// Fetches the remote document, or `None` when none has been synced.
    pub async fn get_document(
        &self,
        access_token: &str,
    ) -> Result<Option<RemoteDocument>, ApiError> {
        let response = self
            .http
            .get(self.url("/v1/document"))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(classify_transport)?;

        let response = match check_status(response).await {
            Ok(response) => response,
            // "Nothing synced yet" is a state, not a failure.
            Err(ApiError::NotFound) => return Ok(None),
            Err(other) => return Err(other),
        };

        let body: DocumentBody = response.json().await.map_err(|_| ApiError::Malformed)?;
        let bytes = BASE64
            .decode(body.document.as_bytes())
            .map_err(|_| ApiError::Malformed)?;

        Ok(Some(RemoteDocument {
            etag: body.etag,
            updated_at: body.updated_at,
            bytes,
        }))
    }

    /// Writes the document.
    ///
    /// `if_match` is required by the server: `"*"` to write regardless, or the
    /// etag the write is based on.
    pub async fn put_document(
        &self,
        access_token: &str,
        bytes: &[u8],
        if_match: &str,
    ) -> Result<WriteResult, ApiError> {
        let response = self
            .http
            .put(self.url("/v1/document"))
            .bearer_auth(access_token)
            .header("if-match", if_match)
            .json(&DocumentWriteBody {
                document: BASE64.encode(bytes),
            })
            .send()
            .await
            .map_err(classify_transport)?;

        let response = check_status(response).await?;
        response.json().await.map_err(|_| ApiError::Malformed)
    }

    // -- Health -------------------------------------------------------------

    /// Whether the service is reachable. Used by `doctor`.
    pub async fn health(&self) -> Result<bool, ApiError> {
        let response = self
            .http
            .get(self.url("/health"))
            .send()
            .await
            .map_err(classify_transport)?;
        Ok(response.status().is_success())
    }
}

/// Distinguishes a timeout from other transport failures.
fn classify_transport(error: reqwest::Error) -> ApiError {
    if error.is_timeout() {
        ApiError::Timeout
    } else {
        ApiError::Network(error)
    }
}

/// Maps a response status onto the error contract.
async fn check_status(response: reqwest::Response) -> Result<reqwest::Response, ApiError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());

    // The body may be absent or not JSON; neither should mask the status.
    let body: Option<ErrorBody> = response.json().await.ok();
    let code = body.as_ref().map(|b| b.code.clone()).unwrap_or_default();
    let message = body.as_ref().map(|b| b.message.clone()).unwrap_or_default();
    let request_id = body
        .as_ref()
        .and_then(|b| b.request_id.clone())
        .unwrap_or_else(|| "unknown".to_owned());

    Err(match status.as_u16() {
        401 => {
            if code == "SESSION_EXPIRED" {
                ApiError::SessionExpired
            } else {
                ApiError::Unauthenticated
            }
        }
        403 => ApiError::Unauthenticated,
        404 => ApiError::NotFound,
        409 => ApiError::Conflict(Box::new(body.map(|b| b.conflict).unwrap_or_default())),
        429 => ApiError::RateLimited {
            retry_after_seconds: retry_after.unwrap_or(60),
        },
        400 | 413 => ApiError::Rejected { code, message },
        // A service that explained itself is repeating that explanation. This
        // is where "codes cannot be sent right now" and "this service is not
        // ready yet" arrive, and both are worth more than a request id.
        503 if !message.is_empty() => ApiError::Unavailable { message },
        500..=599 => ApiError::Server { request_id },
        _ => ApiError::Rejected { code, message },
    })
}
