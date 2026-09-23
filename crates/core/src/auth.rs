//! Where the GitHub token lives. Only in the OS keychain: never in SQLite,
//! logs, or the webview.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

use crate::error::{Error, Result};

pub trait SecretStore: Send + Sync {
    fn get(&self) -> Result<Option<String>>;
    fn set(&self, token: &str) -> Result<()>;
    fn clear(&self) -> Result<()>;
}

/// For tests, and as a fallback where no keychain is available.
#[derive(Default)]
pub struct MemorySecretStore(Mutex<Option<String>>);

impl SecretStore for MemorySecretStore {
    fn get(&self) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn set(&self, token: &str) -> Result<()> {
        *self.0.lock().unwrap() = Some(token.to_string());
        Ok(())
    }
    fn clear(&self) -> Result<()> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

#[cfg(feature = "keychain")]
pub struct KeychainSecretStore {
    entry: keyring::Entry,
}

#[cfg(feature = "keychain")]
impl KeychainSecretStore {
    pub fn new(service: &str) -> Result<Self> {
        let entry = keyring::Entry::new(service, "github.com")
            .map_err(|e| Error::Internal(format!("keychain: {e}")))?;
        Ok(KeychainSecretStore { entry })
    }
}

#[cfg(feature = "keychain")]
impl SecretStore for KeychainSecretStore {
    fn get(&self) -> Result<Option<String>> {
        match self.entry.get_password() {
            Ok(t) => Ok(Some(t)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Internal(format!("keychain: {e}"))),
        }
    }
    fn set(&self, token: &str) -> Result<()> {
        self.entry.set_password(token).map_err(|e| Error::Internal(format!("keychain: {e}")))
    }
    fn clear(&self) -> Result<()> {
        match self.entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Internal(format!("keychain: {e}"))),
        }
    }
}

/// Finds the `gh` binary. Apps launched from Finder get a minimal PATH
/// without Homebrew, so look in the usual install locations too.
pub fn find_gh() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join(gh_name())).collect())
        .unwrap_or_default();
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/home/linuxbrew/.linuxbrew/bin"] {
        candidates.push(PathBuf::from(dir).join(gh_name()));
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".local/bin").join(gh_name()));
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        candidates.push(PathBuf::from(pf).join("GitHub CLI").join(gh_name()));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn gh_name() -> &'static str {
    if cfg!(windows) { "gh.exe" } else { "gh" }
}

/// Reads the token the GitHub CLI is signed in with (`gh auth token`).
pub fn gh_cli_token() -> Result<String> {
    let gh = find_gh().ok_or_else(|| Error::Invalid("The GitHub CLI (gh) isn't installed.".into()))?;
    let out = Command::new(gh)
        .args(["auth", "token", "--hostname", "github.com"])
        .output()
        .map_err(|e| Error::Invalid(format!("Couldn't run gh: {e}")))?;
    if !out.status.success() {
        return Err(Error::Invalid(
            "The GitHub CLI isn't signed in. Run `gh auth login`, or paste a token instead.".into(),
        ));
    }
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if token.is_empty() {
        return Err(Error::Invalid("gh returned an empty token.".into()));
    }
    Ok(token)
}
