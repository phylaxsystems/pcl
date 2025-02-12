use dirs::home_dir;
use eyre::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

const AUTH_FILE: &str = ".phylax/auth.json";
const BASE_URL: &str = "https://credible-layer-dapp.pages.dev";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const MAX_RETRIES: u32 = 150; // 5 minutes worth of 2-second intervals

const PHYLAX_ASCII: &str = r#"
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@BD>"            "<8@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@BP"     _.               '4@@@@@@@@@@@@@@@
@@@@@@@@@@@@D    _e@@B  ,_,   __  t@g_    `G@@@@@@@@@@@@
@@@@@@@@@@P   _g@@@@P  /@@@  [@@@  \@@@@_    %@@@@@@@@@@
@@@@@@@@B   _B@@@@@W  {@@@@  [@@@@  T@@@@@a   `@@@@@@@@@
@@@@@@@P   g@@@@@@@  ;@@@@@  [@@@@A  @@@@@@@_   f@@@@@@@
@@@@@@P  ,@@@@@@@@F  @@@@@@  g@@@@@  !@@@@@@@L   V@@@@@@
@@@@@B   @@@@@@@@@  ;@@@@@@  B@@@@@|  @@@@@@@@L   @@@@@@
@@@@@'  [@@@@@@@@@  g@@BBD>  <4B@@@@  @@@@@@@@@   '@@@@@
@@@@@   @@@@@@@@@@  BW  __    __ `8@  B@@@@@@@@j   @@@@@
@@@@@                 ;@@@@  B@@@;                 @@@@@
@@@@@   qgg@@@@@@g  __ "B@@  @BB" __  g@@@@@@gq;   @@@@@
@@@@@   @@@@@@@@@@  @@@q___  ___g@@B  @@@@@@@@@   .@@@@@
@@@@@@   @@@@@@@@@  [@@@@@@  @@@@@@|  @@@@@@@@P   g@@@@@
@@@@@@\  '@@@@@@@@,  @@@@@@  @@@@@@  |@@@@@@@W   /@@@@@@
@@@@@@@L  `@@@@@@@@  0@@@@g  @@@@@F  @@@@@@@P   /@@@@@@@
@@@@@@@@p   \@@@@@@,  @@@@8  @@@@W  A@@@@@B    j@@@@@@@@
@@@@@@@@@@_   "@@@@@,  @@@]  @@@D  /@@@@D    _@@@@@@@@@@
@@@@@@@@@@@@_    <B@@_  <=   "8"  /@BP"    _@@@@@@@@@@@@
@@@@@@@@@@@@@@@_     ""          "      _g@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@g__              __g@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
"#;

#[derive(Serialize, Deserialize)]
struct AuthResponse {
    code: String,
    sessionId: String,
    deviceSecret: String,
    expiresAt: String,
}

#[derive(Serialize, Deserialize)]
struct StatusResponse {
    verified: bool,
    address: Option<String>,
    token: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredAuth {
    access_token: String,
    refresh_token: String,
    address: String,
}

pub async fn login() -> Result<()> {
    let client = Client::new();

    // Get authentication code
    let auth_response: AuthResponse = client
        .get(format!("{}/api/v1/cli/auth/code", BASE_URL))
        .send()
        .await?
        .json()
        .await?;

    // Display code and instructions to user
    println!("\n🔑 Your authentication code is:\n");
    println!("    ┌──────────┐");
    println!("    │  {}  │", auth_response.code);
    println!("    └──────────┘\n");
    println!("📱 Please visit:");
    println!(
        "   {}/device?session_id={}\n",
        BASE_URL, auth_response.sessionId
    );
    println!("⏳ Waiting for authentication...\n");

    // Poll for authentication status
    let mut attempts = 0;
    while attempts < MAX_RETRIES {
        let status: StatusResponse = client
            .get(format!("{}/api/v1/cli/auth/status", BASE_URL))
            .query(&[
                ("session_id", &auth_response.sessionId),
                ("device_secret", &auth_response.deviceSecret),
            ])
            .send()
            .await?
            .json()
            .await?;

        if status.verified {
            // Store authentication tokens
            let auth = StoredAuth {
                access_token: status.token.unwrap(),
                refresh_token: status.refresh_token.unwrap(),
                address: status.address.unwrap(),
            };
            store_auth(&auth)?;
            println!("{}", PHYLAX_ASCII);
            println!("\n🎉 Authentication successful!");
            println!("🔗 Connected wallet: {}\n", auth.address);
            return Ok(());
        }

        attempts += 1;
        sleep(POLL_INTERVAL).await;
    }

    Err(eyre::eyre!("Authentication timed out"))
}

pub fn logout() -> Result<()> {
    let auth_path = get_auth_path()?;
    if auth_path.exists() {
        fs::remove_file(auth_path)?;
    }
    println!("👋 Logged out successfully");
    Ok(())
}

pub fn status() -> Result<()> {
    match read_auth()? {
        Some(auth) => {
            println!("✅ Logged in as: {}", auth.address);
        }
        None => {
            println!("❌ Not logged in");
        }
    }
    Ok(())
}

fn store_auth(auth: &StoredAuth) -> Result<()> {
    let auth_path = get_auth_path()?;
    if let Some(parent) = auth_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(auth_path, serde_json::to_string(auth)?)?;
    Ok(())
}

fn read_auth() -> Result<Option<StoredAuth>> {
    let auth_path = get_auth_path()?;
    if !auth_path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(auth_path)?;
    Ok(Some(serde_json::from_str(&contents)?))
}

fn get_auth_path() -> Result<PathBuf> {
    let home = home_dir().ok_or_else(|| eyre::eyre!("Could not find home directory"))?;
    Ok(home.join(AUTH_FILE))
}
