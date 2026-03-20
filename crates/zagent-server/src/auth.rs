use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use reqwest::StatusCode;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const OPENAI_AUTH_ISSUER: &str = "https://auth.openai.com";
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEVICE_VERIFICATION_URL: &str = "https://auth.openai.com/codex/device";
const DEVICE_CALLBACK_URL: &str = "https://auth.openai.com/deviceauth/callback";
const DEVICE_CODE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_auth_id: String,
    #[serde(alias = "user_code", alias = "usercode")]
    user_code: String,
    #[serde(default, deserialize_with = "deserialize_interval")]
    interval: u64,
}

#[derive(Debug, Deserialize)]
struct DeviceCodePollSuccess {
    authorization_code: String,
    code_challenge: String,
    code_verifier: String,
}

#[derive(Debug, Deserialize)]
struct TokenExchangeResponse {
    id_token: String,
    access_token: String,
}

#[derive(Debug, Serialize)]
struct DeviceCodeRequest<'a> {
    client_id: &'a str,
}

#[derive(Debug, Serialize)]
struct DeviceCodePollRequest<'a> {
    device_auth_id: &'a str,
    user_code: &'a str,
}

#[derive(Debug, Serialize)]
struct AuthFile<'a> {
    providers: AuthFileProviders<'a>,
}

#[derive(Debug, Serialize)]
struct AuthFileProviders<'a> {
    openai: AuthFileOpenAi<'a>,
}

#[derive(Debug, Serialize)]
struct AuthFileOpenAi<'a> {
    auth_method: &'a str,
    access_token: &'a str,
    account_id: &'a str,
}

#[derive(Debug)]
pub struct OpenAiAuthOutput {
    pub access_token: String,
    pub account_id: String,
}

pub async fn run_openai_auth(
    auth_file: &Path,
) -> Result<OpenAiAuthOutput, Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder().build()?;
    let device = request_device_code(&client).await?;

    println!("Open this URL in your browser:");
    println!("{DEVICE_VERIFICATION_URL}");
    println!();
    println!("Enter this one-time code:");
    println!("{}", device.user_code);
    println!();
    println!("Waiting for authorization...");

    let code = poll_for_authorization_code(&client, &device).await?;
    let pkce = PkceCodes {
        code_verifier: code.code_verifier,
        code_challenge: code.code_challenge,
    };
    let tokens = exchange_code_for_tokens(&client, &pkce, &code.authorization_code).await?;
    let account_id = parse_chatgpt_account_id(&tokens.id_token)?;

    let output = OpenAiAuthOutput {
        access_token: tokens.access_token,
        account_id,
    };
    write_auth_file(auth_file, &output)?;
    Ok(output)
}

async fn request_device_code(
    client: &reqwest::Client,
) -> Result<DeviceCodeResponse, Box<dyn std::error::Error>> {
    let response = client
        .post(format!(
            "{OPENAI_AUTH_ISSUER}/api/accounts/deviceauth/usercode"
        ))
        .json(&DeviceCodeRequest {
            client_id: OPENAI_CLIENT_ID,
        })
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!(
            "OpenAI device-code request failed with status {}",
            response.status()
        )
        .into());
    }

    Ok(response.json().await?)
}

async fn poll_for_authorization_code(
    client: &reqwest::Client,
    device: &DeviceCodeResponse,
) -> Result<DeviceCodePollSuccess, Box<dyn std::error::Error>> {
    let interval = device.interval.max(1);
    let start = Instant::now();

    loop {
        let response = client
            .post(format!(
                "{OPENAI_AUTH_ISSUER}/api/accounts/deviceauth/token"
            ))
            .json(&DeviceCodePollRequest {
                device_auth_id: &device.device_auth_id,
                user_code: &device.user_code,
            })
            .send()
            .await?;

        if response.status().is_success() {
            return Ok(response.json().await?);
        }

        if matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ) {
            if start.elapsed() >= DEVICE_CODE_TIMEOUT {
                return Err("OpenAI device auth timed out after 15 minutes".into());
            }
            tokio::time::sleep(Duration::from_secs(interval)).await;
            continue;
        }

        return Err(format!(
            "OpenAI device auth failed with status {}",
            response.status()
        )
        .into());
    }
}

async fn exchange_code_for_tokens(
    client: &reqwest::Client,
    pkce: &PkceCodes,
    authorization_code: &str,
) -> Result<TokenExchangeResponse, Box<dyn std::error::Error>> {
    let response = client
        .post(format!("{OPENAI_AUTH_ISSUER}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
            urlencoding::encode(authorization_code),
            urlencoding::encode(DEVICE_CALLBACK_URL),
            urlencoding::encode(OPENAI_CLIENT_ID),
            urlencoding::encode(&pkce.code_verifier)
        ))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!(
            "OpenAI token exchange failed with status {}",
            response.status()
        )
        .into());
    }

    let tokens: TokenExchangeResponse = response.json().await?;
    let expected_challenge = generate_code_challenge(&pkce.code_verifier);
    if tokens.id_token.trim().is_empty() || tokens.access_token.trim().is_empty() {
        return Err("OpenAI token exchange returned empty tokens".into());
    }
    if expected_challenge != pkce.code_challenge {
        return Err("OpenAI device auth returned inconsistent PKCE codes".into());
    }
    Ok(tokens)
}

fn parse_chatgpt_account_id(id_token: &str) -> Result<String, Box<dyn std::error::Error>> {
    let payload = id_token
        .split('.')
        .nth(1)
        .ok_or("OpenAI ID token did not contain a JWT payload")?;
    let decoded = URL_SAFE_NO_PAD.decode(payload)?;
    let claims: Value = serde_json::from_slice(&decoded)?;
    claims
        .get("chatgpt_account_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| "OpenAI ID token did not include chatgpt_account_id".into())
}

fn deserialize_interval<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(raw) => raw.trim().parse::<u64>().map_err(de::Error::custom),
        Value::Number(raw) => raw
            .as_u64()
            .ok_or_else(|| de::Error::custom("interval number must be positive")),
        _ => Err(de::Error::custom("interval must be a string or number")),
    }
}

fn write_auth_file(
    auth_file: &Path,
    output: &OpenAiAuthOutput,
) -> Result<(), Box<dyn std::error::Error>> {
    let body = render_auth_file(output)?;
    fs::write(auth_file, body)?;
    Ok(())
}

fn render_auth_file(output: &OpenAiAuthOutput) -> Result<String, Box<dyn std::error::Error>> {
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&AuthFile {
            providers: AuthFileProviders {
                openai: AuthFileOpenAi {
                    auth_method: "chatgpt_subscription",
                    access_token: &output.access_token,
                    account_id: &output.account_id,
                },
            },
        })?
    ))
}

#[derive(Debug)]
struct PkceCodes {
    code_verifier: String,
    code_challenge: String,
}

#[allow(dead_code)]
fn generate_pkce() -> PkceCodes {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let code_verifier = URL_SAFE_NO_PAD.encode(bytes);
    let code_challenge = generate_code_challenge(&code_verifier);
    PkceCodes {
        code_verifier,
        code_challenge,
    }
}

fn generate_code_challenge(code_verifier: &str) -> String {
    let digest = Sha256::digest(code_verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_account_id_from_jwt_payload() {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(br#"{"chatgpt_account_id":"acct_123"}"#);
        let token = format!("{header}.{payload}.sig");

        assert_eq!(
            parse_chatgpt_account_id(&token).expect("account id"),
            "acct_123"
        );
    }

    #[test]
    fn renders_auth_file() {
        let body = render_auth_file(&OpenAiAuthOutput {
            access_token: "token".to_string(),
            account_id: "acct_123".to_string(),
        })
        .expect("auth file");

        assert!(body.contains("\"providers\""));
        assert!(body.contains("\"openai\""));
        assert!(body.contains("\"auth_method\": \"chatgpt_subscription\""));
        assert!(body.contains("\"access_token\": \"token\""));
        assert!(body.contains("\"account_id\": \"acct_123\""));
    }
}
