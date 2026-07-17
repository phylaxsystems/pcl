//! Local wallet support for signing transactions and challenges.
//!
//! The dapp performs these signatures in the user's browser wallet; pcl
//! replicates them with an alloy local signer sourced from a raw private key
//! or a foundry-compatible encrypted keystore.

use alloy_primitives::hex;
use alloy_signer::Signer;
use alloy_signer_local::PrivateKeySigner;
use dirs::home_dir;
use std::path::PathBuf;
use thiserror::Error;

/// Directory under `$HOME` where foundry stores encrypted keystores.
const FOUNDRY_KEYSTORES_DIR: &str = ".foundry/keystores";

/// Errors that can occur while resolving a signer or producing signatures.
#[derive(Error, Debug)]
pub enum WalletError {
    #[error(
        "No wallet configured. Pass --private-key (or set PCL_PRIVATE_KEY), or --account <keystore-name>."
    )]
    NoWallet,

    #[error("Invalid private key: {0}")]
    InvalidPrivateKey(String),

    #[error("Keystore not found at {}", .0.display())]
    KeystoreNotFound(PathBuf),

    #[error(
        "Keystore password required. Pass --keystore-password / --keystore-password-file, or set PCL_KEYSTORE_PASSWORD / PCL_KEYSTORE_PASSWORD_FILE (interactive prompt is unavailable in --json mode or without a TTY)."
    )]
    PasswordRequired,

    #[error("Failed to read keystore password: {0}")]
    PasswordPrompt(#[source] std::io::Error),

    #[error("Failed to decrypt keystore {}: {source}", .path.display())]
    KeystoreDecrypt {
        path: PathBuf,
        #[source]
        source: alloy_signer_local::LocalSignerError,
    },

    #[error("Signing failed: {0}")]
    Signing(#[source] alloy_signer::Error),

    #[error("Keystore decryption task failed: {0}")]
    Join(#[source] tokio::task::JoinError),
}

/// Wallet selection arguments shared by every command that signs.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct WalletArgs {
    /// Hex-encoded private key used to sign transactions and challenges
    #[arg(
        long,
        env = "PCL_PRIVATE_KEY",
        hide_env_values = true,
        conflicts_with = "account"
    )]
    pub private_key: Option<String>,

    /// Name of a foundry keystore in ~/.foundry/keystores (see `cast wallet import`)
    #[arg(long)]
    pub account: Option<String>,

    /// Password for the foundry keystore
    #[arg(long, env = "PCL_KEYSTORE_PASSWORD", hide_env_values = true)]
    pub keystore_password: Option<String>,

    /// File containing the keystore password (foundry --password-file style)
    #[arg(
        long,
        env = "PCL_KEYSTORE_PASSWORD_FILE",
        conflicts_with = "keystore_password"
    )]
    pub keystore_password_file: Option<PathBuf>,

    /// Directory containing foundry keystores (defaults to ~/.foundry/keystores)
    #[arg(long, hide = true)]
    pub keystores_dir: Option<PathBuf>,

    /// Signer resolved ahead of time, set programmatically (never from the
    /// CLI). Lets an orchestrator decrypt a keystore once — prompting for the
    /// password a single time — and reuse the signer across sub-flows.
    #[arg(skip)]
    pub resolved: Option<PrivateKeySigner>,
}

impl WalletArgs {
    /// Wraps an already-resolved signer so downstream flows skip keystore
    /// decryption and password prompts entirely.
    pub fn from_signer(signer: PrivateKeySigner) -> Self {
        Self {
            resolved: Some(signer),
            ..Self::default()
        }
    }
}

impl WalletArgs {
    /// Whether the user supplied any wallet source.
    pub fn is_configured(&self) -> bool {
        self.private_key.is_some() || self.account.is_some() || self.resolved.is_some()
    }

    /// Resolves the configured wallet into a signer.
    ///
    /// `interactive` controls whether a missing keystore password may be
    /// prompted for on the terminal; machine-output callers must pass `false`.
    pub async fn signer(&self, interactive: bool) -> Result<PrivateKeySigner, WalletError> {
        if let Some(signer) = &self.resolved {
            return Ok(signer.clone());
        }

        if let Some(key) = &self.private_key {
            return key
                .trim()
                .parse::<PrivateKeySigner>()
                .map_err(|e| WalletError::InvalidPrivateKey(e.to_string()));
        }

        let Some(account) = &self.account else {
            return Err(WalletError::NoWallet);
        };

        let keystores_dir = match &self.keystores_dir {
            Some(dir) => dir.clone(),
            None => {
                home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(FOUNDRY_KEYSTORES_DIR)
            }
        };
        let keystore_path = keystores_dir.join(account);
        if !keystore_path.is_file() {
            return Err(WalletError::KeystoreNotFound(keystore_path));
        }

        let password = match (&self.keystore_password, &self.keystore_password_file) {
            (Some(password), _) => password.clone(),
            (None, Some(file)) => {
                std::fs::read_to_string(file)
                    .map_err(WalletError::PasswordPrompt)?
                    .trim_end_matches(['\r', '\n'])
                    .to_string()
            }
            (None, None) if interactive => {
                rpassword::prompt_password(format!("Password for keystore {account:?}: "))
                    .map_err(WalletError::PasswordPrompt)?
            }
            (None, None) => return Err(WalletError::PasswordRequired),
        };

        // scrypt key derivation is CPU-heavy; keep it off the async runtime.
        let path = keystore_path.clone();
        tokio::task::spawn_blocking(move || PrivateKeySigner::decrypt_keystore(&path, password))
            .await
            .map_err(WalletError::Join)?
            .map_err(|source| {
                WalletError::KeystoreDecrypt {
                    path: keystore_path,
                    source,
                }
            })
    }
}

/// Signs a message with EIP-191 `personal_sign` semantics, matching what
/// wagmi/viem's `signMessage` produces in the dapp.
///
/// Returns the 65-byte r‖s‖v signature as a 0x-prefixed hex string.
pub async fn sign_personal(
    signer: &PrivateKeySigner,
    message: &str,
) -> Result<String, WalletError> {
    let signature = signer
        .sign_message(message.as_bytes())
        .await
        .map_err(WalletError::Signing)?;
    Ok(hex::encode_prefixed(signature.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;
    use alloy_signer::SignerSync;

    /// anvil's well-known account 0.
    const ANVIL_KEY_0: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const ANVIL_ADDR_0: alloy_primitives::Address =
        address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");

    fn key_args(key: &str) -> WalletArgs {
        WalletArgs {
            private_key: Some(key.to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn private_key_with_0x_prefix_resolves_to_expected_address() {
        let signer = key_args(ANVIL_KEY_0).signer(false).await.unwrap();
        assert_eq!(signer.address(), ANVIL_ADDR_0);
    }

    #[tokio::test]
    async fn private_key_without_0x_prefix_resolves_to_expected_address() {
        let signer = key_args(ANVIL_KEY_0.trim_start_matches("0x"))
            .signer(false)
            .await
            .unwrap();
        assert_eq!(signer.address(), ANVIL_ADDR_0);
    }

    #[tokio::test]
    async fn surrounding_whitespace_is_tolerated() {
        let signer = key_args(&format!("  {ANVIL_KEY_0}\n"))
            .signer(false)
            .await
            .unwrap();
        assert_eq!(signer.address(), ANVIL_ADDR_0);
    }

    #[tokio::test]
    async fn invalid_private_key_is_rejected() {
        let err = key_args("0x1234").signer(false).await.unwrap_err();
        assert!(matches!(err, WalletError::InvalidPrivateKey(_)));
    }

    #[tokio::test]
    async fn no_wallet_source_errors() {
        let err = WalletArgs::default().signer(false).await.unwrap_err();
        assert!(matches!(err, WalletError::NoWallet));
        assert!(!WalletArgs::default().is_configured());
    }

    #[tokio::test]
    async fn personal_sign_recovers_to_signer_address() {
        let signer = key_args(ANVIL_KEY_0).signer(false).await.unwrap();
        let message = "phylax credible layer challenge: nonce=42";

        let hex_signature = sign_personal(&signer, message).await.unwrap();
        let raw = hex::decode(&hex_signature).unwrap();
        assert_eq!(raw.len(), 65);

        // Recover through alloy's independent EIP-191 path to prove the
        // prefixing matches wagmi/viem `signMessage` semantics.
        let signature = alloy_primitives::Signature::try_from(raw.as_slice()).unwrap();
        let recovered = signature
            .recover_address_from_msg(message.as_bytes())
            .unwrap();
        assert_eq!(recovered, ANVIL_ADDR_0);
    }

    #[tokio::test]
    async fn personal_sign_matches_sync_signer_output() {
        let signer = key_args(ANVIL_KEY_0).signer(false).await.unwrap();
        let message = "consistency check";
        let expected = signer.sign_message_sync(message.as_bytes()).unwrap();
        let actual = sign_personal(&signer, message).await.unwrap();
        assert_eq!(actual, hex::encode_prefixed(expected.as_bytes()));
    }

    #[tokio::test]
    async fn missing_keystore_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let args = WalletArgs {
            account: Some("nope".to_string()),
            keystore_password: Some("pw".to_string()),
            keystores_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let err = args.signer(false).await.unwrap_err();
        assert!(matches!(err, WalletError::KeystoreNotFound(_)));
    }

    #[tokio::test]
    async fn keystore_password_required_in_machine_mode() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("acct"), "{}").unwrap();
        let args = WalletArgs {
            account: Some("acct".to_string()),
            keystores_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let err = args.signer(false).await.unwrap_err();
        assert!(matches!(err, WalletError::PasswordRequired));
    }

    #[tokio::test]
    async fn keystore_password_file_is_read_and_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let secret = key_args(ANVIL_KEY_0).signer(false).await.unwrap();
        let mut rng = rand::thread_rng();
        PrivateKeySigner::encrypt_keystore(
            dir.path(),
            &mut rng,
            secret.to_bytes(),
            "hunter2",
            Some("file-account"),
        )
        .unwrap();
        let password_file = dir.path().join("password.txt");
        std::fs::write(&password_file, "hunter2\n").unwrap();

        let args = WalletArgs {
            account: Some("file-account".to_string()),
            keystore_password_file: Some(password_file),
            keystores_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let signer = args.signer(false).await.unwrap();
        assert_eq!(signer.address(), ANVIL_ADDR_0);
    }

    #[tokio::test]
    async fn resolved_signer_short_circuits_keystore_and_password() {
        let secret = key_args(ANVIL_KEY_0).signer(false).await.unwrap();
        // Account points at a keystore that doesn't exist and no password is
        // available anywhere: proves the resolved signer wins outright.
        let args = WalletArgs {
            account: Some("missing".to_string()),
            keystores_dir: Some(PathBuf::from("/nonexistent")),
            ..WalletArgs::from_signer(secret)
        };
        assert!(args.is_configured());
        let signer = args.signer(false).await.unwrap();
        assert_eq!(signer.address(), ANVIL_ADDR_0);
    }

    #[tokio::test]
    async fn foundry_keystore_roundtrip_decrypts_to_same_address() {
        let dir = tempfile::tempdir().unwrap();
        let secret = key_args(ANVIL_KEY_0).signer(false).await.unwrap();
        // Written through the same eth-keystore format foundry uses for
        // `cast wallet import`.
        let mut rng = rand::thread_rng();
        PrivateKeySigner::encrypt_keystore(
            dir.path(),
            &mut rng,
            secret.to_bytes(),
            "hunter2",
            Some("e2e-account"),
        )
        .unwrap();

        let args = WalletArgs {
            account: Some("e2e-account".to_string()),
            keystore_password: Some("hunter2".to_string()),
            keystores_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let signer = args.signer(false).await.unwrap();
        assert_eq!(signer.address(), ANVIL_ADDR_0);

        let wrong_password = WalletArgs {
            keystore_password: Some("wrong".to_string()),
            ..args
        };
        let err = wrong_password.signer(false).await.unwrap_err();
        assert!(matches!(err, WalletError::KeystoreDecrypt { .. }));
    }
}
