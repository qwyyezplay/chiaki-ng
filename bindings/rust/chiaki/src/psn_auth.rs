// SPDX-License-Identifier: LicenseRef-AGPL-3.0-only-OpenSSL

//! PSN OAuth2 authentication: token management and Account ID retrieval.
//!
//! This module mirrors the functionality of `psntoken.cpp` and `psnaccountid.cpp`
//! from the GUI layer, providing a pure-Rust synchronous implementation.
//!
//! # Flow
//!
//! 1. Direct the user's browser to [`login_url()`].
//! 2. Capture the `code` query parameter from the redirect to
//!    `https://remoteplay.dl.playstation.net/remoteplay/redirect`.
//! 3. To get tokens only:  [`PsnToken::init(code)`]
//! 4. To get tokens **and** the PSN Account ID: [`PsnAccountId::get(code)`]
//! 5. When the access token expires, call [`PsnToken::refresh(refresh_token)`].

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;

// ---------------------------------------------------------------------------
// OAuth2 constants
// ---------------------------------------------------------------------------

/// PSN Remote Play OAuth2 client identifier.
pub const CLIENT_ID: &str = "ba495a24-818c-472b-b12d-ff231c1b5745";

const CLIENT_SECRET: &str = "mvaiZkRsAsI1IBkY";

/// Endpoint used for both token exchange and token refresh.
pub const TOKEN_URL: &str =
    "https://auth.api.sonyentertainmentnetwork.com/2.0/oauth/token";

const REDIRECT_URI: &str =
    "https://remoteplay.dl.playstation.net/remoteplay/redirect";

const SCOPE: &str = "psn:clientapp \
    referenceDataService:countryConfig.read \
    pushNotification:webSocket.desktop.connect \
    sessionManager:remotePlaySession.system.update";

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during PSN authentication.
#[derive(Debug, thiserror::Error)]
pub enum PsnAuthError {
    /// A network or TLS error from the HTTP client.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The server response could not be parsed as JSON.
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    /// An expected field was absent from the server response.
    #[error("missing field in response: {0}")]
    MissingField(&'static str),

    /// The `user_id` field in the account-info response was not a valid integer.
    #[error("invalid user_id value: {0}")]
    InvalidUserId(String),

    /// The server returned HTTP 400 or 401 — credentials are invalid or expired.
    #[error("unauthorized: invalid or expired credentials")]
    Unauthorized,

    /// The server returned an unexpected non-2xx status code.
    #[error("server returned HTTP {0}")]
    HttpStatus(u16),
}

// ---------------------------------------------------------------------------
// Token response
// ---------------------------------------------------------------------------

/// Tokens returned by the PSN authorization server.
#[derive(Debug, Clone)]
pub struct PsnTokenResponse {
    /// Short-lived bearer token for PSN API requests.
    pub access_token: String,
    /// Long-lived token used to obtain a new `access_token`.
    pub refresh_token: String,
    /// Number of seconds until `access_token` expires.
    pub expires_in: u64,
}

// ---------------------------------------------------------------------------
// PsnToken — token management
// ---------------------------------------------------------------------------

/// Handles PSN access-token acquisition and renewal.
///
/// Mirrors `PSNToken` from `psntoken.cpp`.
pub struct PsnToken;

impl PsnToken {
    /// Exchange an OAuth2 authorization code for access and refresh tokens.
    ///
    /// `redirect_code` is the `code` query parameter captured from the
    /// browser redirect after the user completes the PSN login flow.
    pub fn init(redirect_code: &str) -> Result<PsnTokenResponse, PsnAuthError> {
        let body = format!(
            "grant_type=authorization_code&code={redirect_code}\
             &scope={SCOPE}&redirect_uri={REDIRECT_URI}&"
        );
        request_tokens(&body)
    }

    /// Exchange a refresh token for a new set of tokens.
    ///
    /// Call this when the stored `access_token` has expired.
    pub fn refresh(refresh_token: &str) -> Result<PsnTokenResponse, PsnAuthError> {
        let body = format!(
            "grant_type=refresh_token&refresh_token={refresh_token}\
             &scope={SCOPE}&redirect_uri={REDIRECT_URI}&"
        );
        request_tokens(&body)
    }
}

// ---------------------------------------------------------------------------
// PsnAccountId — token + account-ID retrieval
// ---------------------------------------------------------------------------

/// Retrieves PSN tokens **and** the user's numeric Account ID in one call.
///
/// Mirrors `PSNAccountID` from `psnaccountid.cpp`.
pub struct PsnAccountId;

impl PsnAccountId {
    /// Exchange an authorization code for tokens, then fetch the PSN Account ID.
    ///
    /// Returns `(account_id_bytes, tokens)` where `account_id_bytes` is the
    /// 8-byte **little-endian** representation of the numeric user ID — the
    /// same encoding used by the C++ GUI layer.  Base64-encode the bytes for
    /// persistent storage:
    ///
    /// ```no_run
    /// use base64::{engine::general_purpose::STANDARD, Engine as _};
    /// use chiaki::psn_auth::PsnAccountId;
    ///
    /// let (id_bytes, tokens) = PsnAccountId::get("my_redirect_code")?;
    /// let id_b64 = STANDARD.encode(id_bytes);
    /// # Ok::<_, chiaki::psn_auth::PsnAuthError>(())
    /// ```
    pub fn get(
        redirect_code: &str,
    ) -> Result<([u8; 8], PsnTokenResponse), PsnAuthError> {
        let tokens = PsnToken::init(redirect_code)?;

        let account_info_url = format!("{TOKEN_URL}/{}", tokens.access_token);
        let resp = reqwest::blocking::Client::new()
            .get(&account_info_url)
            .header(AUTHORIZATION, basic_auth_header())
            .header(CONTENT_TYPE, "application/json")
            .send()?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::BAD_REQUEST
        {
            return Err(PsnAuthError::Unauthorized);
        }
        if !status.is_success() {
            return Err(PsnAuthError::HttpStatus(status.as_u16()));
        }

        let json: Value = resp.json()?;
        let user_id_str = json["user_id"]
            .as_str()
            .ok_or(PsnAuthError::MissingField("user_id"))?
            .to_owned();

        let user_id: i64 = user_id_str
            .parse()
            .map_err(|_| PsnAuthError::InvalidUserId(user_id_str))?;

        Ok((user_id.to_le_bytes(), tokens))
    }
}

// ---------------------------------------------------------------------------
// Login URL helper
// ---------------------------------------------------------------------------

/// Build the browser-facing OAuth2 authorization URL.
///
/// Direct the user to this URL to begin the PSN login flow.  After the user
/// authenticates, PSN will redirect the browser to `REDIRECT_URI` with a
/// `code` query parameter that you pass to [`PsnToken::init`] or
/// [`PsnAccountId::get`].
pub fn login_url() -> String {
    format!(
        "https://auth.api.sonyentertainmentnetwork.com/2.0/oauth/authorize\
         ?service_entity=urn:service-entity:psn\
         &response_type=code\
         &client_id={CLIENT_ID}\
         &redirect_uri={REDIRECT_URI}\
         &scope={SCOPE}\
         &request_locale=en_US\
         &ui=pr\
         &service_logo=ps\
         &layout_type=popup\
         &smcid=remoteplay\
         &prompt=always\
         &PlatformPrivacyWs1=minimal&"
    )
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Generate a `Basic …` Authorization header from the PSN client credentials.
fn basic_auth_header() -> String {
    format!(
        "Basic {}",
        B64.encode(format!("{CLIENT_ID}:{CLIENT_SECRET}"))
    )
}

/// Send a form-encoded POST to [`TOKEN_URL`] and return parsed tokens.
fn request_tokens(body: &str) -> Result<PsnTokenResponse, PsnAuthError> {
    let resp = reqwest::blocking::Client::new()
        .post(TOKEN_URL)
        .header(AUTHORIZATION, basic_auth_header())
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body.to_owned())
        .send()?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::BAD_REQUEST
    {
        return Err(PsnAuthError::Unauthorized);
    }
    if !status.is_success() {
        return Err(PsnAuthError::HttpStatus(status.as_u16()));
    }

    let json: Value = resp.json()?;
    parse_token_response(&json)
}

/// Extract [`PsnTokenResponse`] fields from a JSON value.
fn parse_token_response(json: &Value) -> Result<PsnTokenResponse, PsnAuthError> {
    Ok(PsnTokenResponse {
        access_token: json["access_token"]
            .as_str()
            .ok_or(PsnAuthError::MissingField("access_token"))?
            .to_owned(),
        refresh_token: json["refresh_token"]
            .as_str()
            .ok_or(PsnAuthError::MissingField("refresh_token"))?
            .to_owned(),
        expires_in: json["expires_in"]
            .as_u64()
            .ok_or(PsnAuthError::MissingField("expires_in"))?,
    })
}
