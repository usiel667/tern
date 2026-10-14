//! Google OAuth2 for Gmail (IMAP/SMTP via XOAUTH2).
//!
//! Google requires each app to have its own OAuth client. Create a "Desktop app"
//! client in Google Cloud Console, enable the Gmail API, add the
//! `https://mail.google.com/` scope, then set `TERN_GOOGLE_CLIENT_ID` and
//! `TERN_GOOGLE_CLIENT_SECRET` (the desktop "secret" is not confidential; Google
//! documents this). While the consent screen is in "Testing", add your own
//! address as a test user.

use chrono::{Duration, Utc};
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    RedirectUrl, RefreshToken, Scope, TokenResponse, TokenUrl,
};
use tern_core::{Error, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::OAuthTokens;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://mail.google.com/";

fn client_creds() -> Result<(String, Option<String>)> {
    let id = std::env::var("TERN_GOOGLE_CLIENT_ID").map_err(|_| {
        Error::Config("set TERN_GOOGLE_CLIENT_ID (see tern-auth/src/google.rs)".into())
    })?;
    Ok((id, std::env::var("TERN_GOOGLE_CLIENT_SECRET").ok()))
}

fn http() -> Result<oauth2::reqwest::Client> {
    oauth2::reqwest::ClientBuilder::new()
        // Token endpoints must not follow redirects (SSRF guard, per oauth2 docs).
        .redirect(oauth2::reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| Error::Network(e.to_string()))
}

/// Interactive sign-in. `open_browser` is called with the consent URL (the app
/// passes `gtk::UriLauncher`; tests/CLI can use `xdg-open`). Returns tokens
/// once the user approves.
pub async fn sign_in(login_hint: &str, open_browser: impl FnOnce(String)) -> Result<OAuthTokens> {
    let (id, secret) = client_creds()?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    let port = listener.local_addr().map_err(|e| Error::Network(e.to_string()))?.port();
    let redirect = format!("http://127.0.0.1:{port}");

    let mut client = BasicClient::new(ClientId::new(id))
        .set_auth_uri(AuthUrl::new(AUTH_URL.into()).unwrap())
        .set_token_uri(TokenUrl::new(TOKEN_URL.into()).unwrap())
        .set_redirect_uri(RedirectUrl::new(redirect).map_err(|e| Error::Config(e.to_string()))?);
    if let Some(s) = secret {
        client = client.set_client_secret(ClientSecret::new(s));
    }

    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, csrf) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new(SCOPE.into()))
        .add_extra_param("access_type", "offline")
        .add_extra_param("prompt", "consent")
        .add_extra_param("login_hint", login_hint)
        .set_pkce_challenge(challenge)
        .url();
    open_browser(url.to_string());

    let (code, state) = wait_for_redirect(&listener).await?;
    if state != *csrf.secret() {
        return Err(Error::Auth("OAuth state mismatch".into()));
    }
    let token = client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(verifier)
        .request_async(&http()?)
        .await
        .map_err(|e| Error::Auth(format!("token exchange failed: {e}")))?;
    Ok(OAuthTokens {
        access_token: token.access_token().secret().clone(),
        refresh_token: token.refresh_token().map(|r| r.secret().clone()),
        expires_at: token
            .expires_in()
            .map(|d| Utc::now() + Duration::seconds(d.as_secs() as i64)),
    })
}

pub async fn refresh(tokens: &OAuthTokens) -> Result<OAuthTokens> {
    let refresh_token = tokens
        .refresh_token
        .clone()
        .ok_or_else(|| Error::Auth("no refresh token; sign in again".into()))?;
    let (id, secret) = client_creds()?;
    let mut client = BasicClient::new(ClientId::new(id))
        .set_token_uri(TokenUrl::new(TOKEN_URL.into()).unwrap());
    if let Some(s) = secret {
        client = client.set_client_secret(ClientSecret::new(s));
    }
    let token = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token.clone()))
        .request_async(&http()?)
        .await
        .map_err(|e| Error::Auth(format!("token refresh failed: {e}")))?;
    Ok(OAuthTokens {
        access_token: token.access_token().secret().clone(),
        // Google usually omits a new refresh token; keep the old one.
        refresh_token: token
            .refresh_token()
            .map(|r| r.secret().clone())
            .or(Some(refresh_token)),
        expires_at: token
            .expires_in()
            .map(|d| Utc::now() + Duration::seconds(d.as_secs() as i64)),
    })
}

/// Accept one HTTP request on the loopback listener and pull `code`/`state`.
async fn wait_for_redirect(listener: &TcpListener) -> Result<(String, String)> {
    loop {
        let (mut sock, _) = listener.accept().await.map_err(|e| Error::Network(e.to_string()))?;
        let mut buf = vec![0u8; 8192];
        let n = sock.read(&mut buf).await.map_err(|e| Error::Network(e.to_string()))?;
        let req = String::from_utf8_lossy(&buf[..n]);
        let Some(path) = req.lines().next().and_then(|l| l.split_whitespace().nth(1)) else {
            continue;
        };
        let url = url::Url::parse(&format!("http://localhost{path}"))
            .map_err(|e| Error::Auth(e.to_string()))?;
        let get = |k: &str| url.query_pairs().find(|(q, _)| q == k).map(|(_, v)| v.into_owned());
        if let Some(err) = get("error") {
            respond(&mut sock, "Sign-in was cancelled. You can close this tab.").await;
            return Err(Error::Auth(err));
        }
        if let (Some(code), Some(state)) = (get("code"), get("state")) {
            respond(&mut sock, "Signed in to Tern. You can close this tab.").await;
            return Ok((code, state));
        }
        // favicon.ico and friends.
        respond(&mut sock, "").await;
    }
}

async fn respond(sock: &mut tokio::net::TcpStream, msg: &str) {
    let body = format!("<!doctype html><meta charset=utf-8><title>Tern</title><p style=\"font:16px system-ui;margin:3em\">{msg}</p>");
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(resp.as_bytes()).await;
}
