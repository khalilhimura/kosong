//! Talking to the remote service from a synchronous CLI.
//!
//! Two jobs:
//!
//! 1. Own the tokio runtime, so commands stay ordinary synchronous functions
//!    and `main` needs no async machinery.
//! 2. Implement the **one-refresh rule** from §11: on a 401, refresh once,
//!    retry once, then require a fresh sign-in. Retrying a refresh in a loop
//!    against a revoked token family is exactly the pattern the server's reuse
//!    detection treats as an attack, so the client must not do it.

use crate::exit::{CliError, CliResult, Exit};
use kosong_core::api::{ApiClient, ApiError};
use kosong_core::session::{Backend, SessionStore, open_store};
use std::future::Future;

/// Converts an API failure into a CLI failure with the right exit code.
pub fn map_api_error(error: ApiError) -> CliError {
    let repair = error.repair();
    let (exit, code) = match &error {
        ApiError::Network(_) | ApiError::Timeout | ApiError::Server { .. } => {
            (Exit::Network, "NETWORK_ERROR")
        }
        ApiError::Unauthenticated | ApiError::SessionExpired => (Exit::Auth, "NOT_SIGNED_IN"),
        ApiError::NotFound => (Exit::Usage, "NO_REMOTE_DOCUMENT"),
        ApiError::Conflict(_) => (Exit::Usage, "DOCUMENT_CONFLICT"),
        ApiError::RateLimited { .. } => (Exit::Network, "RATE_LIMITED"),
        // A service problem, not the user's — same exit code as a network
        // failure, because from where they are standing it is one.
        ApiError::Unavailable { .. } => (Exit::Network, "SERVICE_UNAVAILABLE"),
        ApiError::Rejected { .. } => (Exit::Usage, "REJECTED"),
        ApiError::Malformed => (Exit::Network, "MALFORMED_RESPONSE"),
    };
    CliError::new(exit, code, error.to_string()).with_repair(repair)
}

/// An authenticated connection to the service.
pub struct Remote {
    client: ApiClient,
    store: Box<dyn SessionStore>,
    runtime: tokio::runtime::Runtime,
}

impl Remote {
    /// Opens a connection. Does not require the user to be signed in.
    pub fn open(base_url: &str) -> CliResult<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| {
                CliError::internal("RUNTIME_FAILED", format!("could not start networking: {e}"))
            })?;

        let client = ApiClient::new(base_url).map_err(map_api_error)?;
        let store = open_store().map_err(|e| {
            let repair = e.repair();
            CliError::new(Exit::Auth, "SESSION_STORE_FAILED", e.to_string()).with_repair(repair)
        })?;

        Ok(Self {
            client,
            store,
            runtime,
        })
    }

    pub fn client(&self) -> &ApiClient {
        &self.client
    }

    pub fn store(&self) -> &dyn SessionStore {
        self.store.as_ref()
    }

    /// Where the refresh token is kept, so the CLI can say so honestly.
    pub fn backend(&self) -> Backend {
        self.store.backend()
    }

    /// Runs a future to completion.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    pub fn is_signed_in(&self) -> bool {
        matches!(self.store.load(), Ok(Some(_)))
    }

    /// Saves a rotated token pair, keeping only the refresh token on disk.
    pub fn remember(&self, refresh_token: &str) -> CliResult<()> {
        self.store.save(refresh_token).map_err(|e| {
            let repair = e.repair();
            CliError::new(Exit::Auth, "SESSION_SAVE_FAILED", e.to_string()).with_repair(repair)
        })
    }

    pub fn forget(&self) -> CliResult<()> {
        self.store.clear().map_err(|e| {
            let repair = e.repair();
            CliError::new(Exit::Auth, "SESSION_CLEAR_FAILED", e.to_string()).with_repair(repair)
        })
    }

    /// Obtains a usable access token, rotating the stored refresh token.
    ///
    /// Access tokens are never written to disk — §8.2 permits only the refresh
    /// token to be stored — so one is minted per command that needs it.
    pub fn access_token(&self) -> CliResult<String> {
        let Some(refresh_token) = self.store.load().map_err(|e| {
            let repair = e.repair();
            CliError::new(Exit::Auth, "SESSION_READ_FAILED", e.to_string()).with_repair(repair)
        })?
        else {
            return Err(not_signed_in());
        };

        let pair =
            self.block_on(self.client.refresh(&refresh_token))
                .map_err(|error| match error {
                    // The stored token is dead. Clearing it means the next command
                    // says "sign in" rather than failing the same way again.
                    ApiError::SessionExpired | ApiError::Unauthenticated => {
                        let _ = self.store.clear();
                        not_signed_in()
                    }
                    other => map_api_error(other),
                })?;

        // The server rotates on every refresh, so the old token is already
        // dead and the new one must be persisted before it is used.
        self.remember(&pair.refresh_token)?;
        Ok(pair.access_token)
    }

    /// Runs an authenticated call, refreshing once if the token has expired.
    ///
    /// The closure receives an access token and is called at most twice.
    pub fn with_access_token<T, F, Fut>(&self, mut call: F) -> CliResult<T>
    where
        F: FnMut(String) -> Fut,
        Fut: Future<Output = Result<T, ApiError>>,
    {
        let token = self.access_token()?;

        match self.block_on(call(token)) {
            Ok(value) => Ok(value),
            Err(error) if error.is_expired_session() => {
                // Exactly one retry. A loop here would look like credential
                // stuffing to the server's reuse detection.
                let retry_token = self.access_token()?;
                self.block_on(call(retry_token)).map_err(map_api_error)
            }
            Err(error) => Err(map_api_error(error)),
        }
    }
}

pub fn not_signed_in() -> CliError {
    CliError::new(Exit::Auth, "NOT_SIGNED_IN", "you are not signed in")
        .with_repair("Run: kosong login")
}

/// A name for this machine, to label the session server-side.
///
/// Best effort and non-identifying: the hostname if the OS offers one,
/// otherwise the platform. Never the username.
pub fn device_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| std::env::consts::OS.to_owned())
}
