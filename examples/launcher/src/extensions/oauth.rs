//! OAuth 2.0 for extensions: the authorization code flow with PKCE and a
//! loopback redirect (RFC 8252), the way a desktop application signs in.
//!
//! The launcher runs the flow so an extension never sees the user's
//! password, only the tokens; it opens the provider's page in the browser,
//! receives the redirect on `127.0.0.1`, exchanges the code, and keeps the
//! tokens in the system keychain under the extension's id and the provider's
//! name.

use std::{
    io::{BufRead as _, BufReader, Write as _},
    net::{TcpListener, TcpStream},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use url::Url;

/// How long the browser has to come back.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// What a provider handed out, as the extension reads it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Unix milliseconds, when the provider said how long the token lasts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

impl Tokens {
    pub fn is_expired(&self, now_ms: u64) -> bool {
        // A minute early, so a request made with it does not fail midway.
        self.expires_at
            .is_some_and(|expires| now_ms + 60_000 >= expires)
    }
}

/// Where to sign in, from the extension.
#[derive(Clone, Debug)]
pub struct Client {
    pub authorize_url: Url,
    pub token_url: Url,
    pub client_id: String,
    pub scope: Option<String>,
    /// Extra query parameters of the authorize URL, such as `prompt`.
    pub extra: Vec<(String, String)>,
    /// A fixed loopback port, for a provider that matches the redirect
    /// exactly; any free port otherwise.
    pub redirect_port: Option<u16>,
}

/// One sign-in in progress: the listener the browser comes back to and the
/// secrets that tie the answer to this request.
pub struct Pending {
    listener: TcpListener,
    verifier: String,
    state: String,
    redirect_uri: String,
    client: Client,
}

impl Pending {
    /// Listens on a free loopback port and builds the URL to open.
    pub fn start(client: Client) -> Result<(Self, Url)> {
        let listener = TcpListener::bind(("127.0.0.1", client.redirect_port.unwrap_or(0)))
            .context("cannot listen for the sign-in")?;
        let redirect_uri = format!(
            "http://127.0.0.1:{}/callback",
            listener.local_addr()?.port()
        );
        let verifier = random_token(48)?;
        let state = random_token(16)?;
        let mut url = client.authorize_url.clone();
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("response_type", "code")
                .append_pair("client_id", &client.client_id)
                .append_pair("redirect_uri", &redirect_uri)
                .append_pair("state", &state)
                .append_pair("code_challenge", &challenge(&verifier))
                .append_pair("code_challenge_method", "S256");
            if let Some(scope) = &client.scope {
                query.append_pair("scope", scope);
            }
            for (name, value) in &client.extra {
                query.append_pair(name, value);
            }
        }
        Ok((
            Self {
                listener,
                verifier,
                state,
                redirect_uri,
                client,
            },
            url,
        ))
    }

    /// Waits for the browser, then exchanges the code for tokens. Blocking.
    pub fn finish(self) -> Result<Tokens> {
        let code = self.receive_code(SIGN_IN_TIMEOUT)?;
        exchange(
            &self.client.token_url,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &self.redirect_uri),
                ("client_id", &self.client.client_id),
                ("code_verifier", &self.verifier),
            ],
        )
    }

    fn receive_code(&self, timeout: Duration) -> Result<String> {
        self.listener.set_nonblocking(true)?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false)?;
                    // A browser may ask for a favicon first; only the
                    // callback answers.
                    if let Some(code) = self.answer(stream)? {
                        return Ok(code);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() > deadline {
                        bail!("the sign-in was not completed in time");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => return Err(error).context("the sign-in failed"),
            }
        }
    }

    fn answer(&self, mut stream: TcpStream) -> Result<Option<String>> {
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line)?;
        let target = line.split_whitespace().nth(1).unwrap_or_default();
        let result = callback_code(target, &self.state);
        let (status, body) = match &result {
            Ok(Some(_)) => ("200 OK", "Signed in. You can close this tab."),
            Ok(None) => ("404 Not Found", ""),
            Err(_) => (
                "400 Bad Request",
                "The sign-in failed. You can close this tab.",
            ),
        };
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .ok();
        result
    }
}

/// The code of a callback request target such as `/callback?code=…&state=…`;
/// `None` for another path, an error for a refusal or a forged state.
fn callback_code(target: &str, state: &str) -> Result<Option<String>> {
    let url = Url::parse(&format!("http://127.0.0.1{target}"))?;
    if url.path() != "/callback" {
        return Ok(None);
    }
    let parameter = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    if let Some(error) = parameter("error") {
        bail!(
            "the provider refused: {}",
            parameter("error_description").unwrap_or(error)
        );
    }
    if parameter("state").as_deref() != Some(state) {
        bail!("the sign-in answer does not belong to this request");
    }
    parameter("code")
        .map(Some)
        .ok_or_else(|| anyhow!("the provider sent no code"))
}

/// Asks for new tokens with a refresh token. Blocking. A provider that does
/// not send a new refresh token keeps the old one valid, so it is kept.
pub fn refresh(token_url: &Url, client_id: &str, previous: &Tokens) -> Result<Tokens> {
    let refresh_token = previous
        .refresh_token
        .as_deref()
        .ok_or_else(|| anyhow!("there is no refresh token; sign in again"))?;
    let mut tokens = exchange(
        token_url,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ],
    )?;
    if tokens.refresh_token.is_none() {
        tokens.refresh_token = previous.refresh_token.clone();
    }
    Ok(tokens)
}

fn exchange(token_url: &Url, form: &[(&str, &str)]) -> Result<Tokens> {
    let response = reqwest::blocking::Client::new()
        .post(token_url.clone())
        .header("Accept", "application/json")
        .form(form)
        .timeout(Duration::from_secs(30))
        .send()
        .with_context(|| format!("cannot reach {}", token_url.host_str().unwrap_or_default()))?;
    let status = response.status();
    let body = response.text()?;
    if !status.is_success() {
        bail!("the provider answered {status}: {}", body.trim());
    }
    parse_tokens(&body, now_ms())
}

fn parse_tokens(body: &str, now_ms: u64) -> Result<Tokens> {
    #[derive(Deserialize)]
    struct Response {
        access_token: String,
        refresh_token: Option<String>,
        id_token: Option<String>,
        scope: Option<String>,
        expires_in: Option<serde_json::Value>,
    }
    let response: Response = serde_json::from_str(body)
        .with_context(|| format!("the provider's answer has no access token: {body}"))?;
    // Some providers send the lifetime as a string.
    let expires_in = response.expires_in.and_then(|value| match value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    });
    Ok(Tokens {
        access_token: response.access_token,
        refresh_token: response.refresh_token,
        id_token: response.id_token,
        scope: response.scope,
        expires_at: expires_in.map(|seconds| now_ms + seconds * 1000),
    })
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// The keychain account holding an extension's tokens for a provider.
pub fn account(extension: &str, provider: &str) -> String {
    format!("oauth/{extension}/{provider}")
}

/// `S256`: the URL-safe Base64 of the verifier's SHA-256.
fn challenge(verifier: &str) -> String {
    base64_url(&Sha256::digest(verifier.as_bytes()))
}

fn random_token(bytes: usize) -> Result<String> {
    let mut buffer = vec![0u8; bytes];
    getrandom::getrandom(&mut buffer).map_err(|error| anyhow!("no randomness: {error}"))?;
    Ok(base64_url(&buffer))
}

/// Base64 with the URL alphabet and no padding (RFC 4648 §5).
fn base64_url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (ix, byte)| {
            n | (u32::from(*byte) << (16 - 8 * ix))
        });
        for ix in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * ix) & 63) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use super::*;

    #[test]
    fn test_pkce_challenge_matches_rfc_7636() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(base64_url(b"f"), "Zg");
        assert_eq!(base64_url(b"fo"), "Zm8");
        assert_eq!(base64_url(b"foo"), "Zm9v");
    }

    #[test]
    fn test_callback_checks_state() {
        assert_eq!(
            callback_code("/callback?code=abc&state=s1", "s1").unwrap(),
            Some("abc".into())
        );
        assert_eq!(callback_code("/favicon.ico", "s1").unwrap(), None);
        assert!(callback_code("/callback?code=abc&state=other", "s1").is_err());
        let error = callback_code(
            "/callback?error=access_denied&error_description=No+thanks",
            "s1",
        )
        .unwrap_err();
        assert!(error.to_string().contains("No thanks"), "{error}");
    }

    #[test]
    fn test_parses_token_answers() {
        let tokens = parse_tokens(
            r#"{"access_token":"a","refresh_token":"r","expires_in":"3600","token_type":"bearer"}"#,
            1_000,
        )
        .unwrap();
        assert_eq!(tokens.expires_at, Some(3_601_000));
        assert!(!tokens.is_expired(1_000));
        assert!(tokens.is_expired(3_560_000));
        assert!(parse_tokens(r#"{"error":"bad"}"#, 0).is_err());
    }

    /// A whole sign-in against a provider of this test's own: the "browser"
    /// follows the redirect, and the token endpoint checks the verifier.
    #[test]
    fn test_signs_in_with_pkce() {
        let provider = TcpListener::bind("127.0.0.1:0").unwrap();
        let token_url = format!("http://{}/token", provider.local_addr().unwrap());
        let client = Client {
            authorize_url: Url::parse("https://provider.example/authorize").unwrap(),
            token_url: Url::parse(&token_url).unwrap(),
            client_id: "launcher-test".into(),
            scope: Some("read".into()),
            extra: vec![("prompt".into(), "consent".into())],
            redirect_port: None,
        };
        let (pending, url) = Pending::start(client).unwrap();
        let query: std::collections::HashMap<String, String> =
            url.query_pairs().into_owned().collect();
        assert_eq!(query["scope"], "read");
        assert_eq!(query["prompt"], "consent");
        let challenge_sent = query["code_challenge"].clone();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = provider.accept().unwrap();
            let mut request = vec![0u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
            let form: std::collections::HashMap<String, String> =
                url::form_urlencoded::parse(body.as_bytes())
                    .into_owned()
                    .collect();
            let answer = if challenge(&form["code_verifier"]) == challenge_sent
                && form["code"] == "the-code"
            {
                r#"{"access_token":"token-1","refresh_token":"refresh-1","expires_in":60}"#
            } else {
                r#"{"error":"invalid_grant"}"#
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{answer}",
                answer.len()
            )
            .unwrap();
        });
        let browser = std::thread::spawn(move || {
            let redirect = format!(
                "{}?code=the-code&state={}",
                query["redirect_uri"], query["state"]
            );
            reqwest::blocking::get(redirect).unwrap().text().unwrap()
        });
        let tokens = pending.finish().unwrap();
        server.join().unwrap();
        assert!(browser.join().unwrap().contains("Signed in"));
        assert_eq!(tokens.access_token, "token-1");
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-1"));
    }
}
