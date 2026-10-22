//! Local encrypted secret store.
//!
//! This file is self-contained so tongyun-git and tongyun-api can copy it.
//! It does not link `keyring`, the Security.framework password APIs, or
//! `tauri-plugin-stronghold`, so saving a secret cannot pop the macOS login
//! keychain password dialog.
//!
//! The encryption key is HKDF-SHA256 over a 32-byte seed stored next to the
//! vault (`secret.key`, mode `0600`). The directory itself is mode `0700` and
//! is rejected if it is a symlink (note: ancestor directories such as
//! `~/.config` are not checked for symlinks; the threat model is within the
//! same local user). The seed is generated locally with `create_new` into a
//! temporary file under a file lock, then atomically linked or renamed and fsync'd.
//! Nothing is derived from the OS keychain or from a machine-id API. A vault
//! without its seed is an error; the seed is never regenerated over an existing
//! vault.
//!
//! # Migration
//!
//! [`FileSecretStore::migrate_from_legacy`] reads each registered legacy item
//! once per attempt. After the marker file is written it never calls the
//! legacy source again, including later setting saves. A failed read or a
//! failed verify deletes nothing and does not write the marker, so the next
//! launch can retry that item. A failed delete still records the account as
//! finished, so a later launch does not touch it again; the encrypted file
//! is the fallback.
//!
//! # Porting the old keychain
//!
//! Implement [`LegacySecretSource`] with the old `keyring` calls and pass the
//! service/account pairs that app used to write. Keep that adapter out of the
//! default build once migration has run (the marker file makes later launches
//! skip it entirely).
//!
//! ```ignore
//! fn read(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError> {
//!     let entry = keyring::Entry::new(service, account).map_err(SecretStoreError::legacy)?;
//!     match entry.get_password() {
//!         Ok(value) => Ok(Some(value)),
//!         Err(keyring::Error::NoEntry) => Ok(None),
//!         Err(err) => Err(SecretStoreError::legacy(err)),
//!     }
//! }
//! ```
//!
//! tongyun-dev itself never stored a setting in the keychain. Project edits
//! write `projects.toml`, the remembered config path writes `settings.json`,
//! and theme / sidebar width stay in `localStorage`. [`migrate_app_secrets`]
//! therefore registers no legacy accounts and does not open a keychain.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::Aead;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroize;

const MAGIC: &[u8] = b"TYSEC1\0\0";
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
const VAULT_VERSION: u32 = 1;
const HKDF_SALT: &[u8] = b"tongyun-secret-store-v1";
const DEFAULT_KDF_INFO: &str = "tongyun/secret-store/v1";
const DEFAULT_KEY_NAME: &str = "secret.key";
const DEFAULT_VAULT_NAME: &str = "secrets.vault";
const DEFAULT_JOURNAL_NAME: &str = "secrets.migration.json";
const DEFAULT_MARKER_NAME: &str = "secrets.migrated";
#[cfg_attr(not(unix), allow(dead_code))]
const LOCK_NAME: &str = ".secrets.lock";

/// Where the seed, vault, migration journal, and done-marker live.
#[derive(Debug, Clone)]
pub struct StorePaths {
    pub dir: PathBuf,
    pub key_name: String,
    pub vault_name: String,
    pub journal_name: String,
    pub marker_name: String,
    /// HKDF `info`. Changing it makes an existing vault unreadable.
    pub kdf_info: String,
}

impl StorePaths {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            key_name: DEFAULT_KEY_NAME.to_string(),
            vault_name: DEFAULT_VAULT_NAME.to_string(),
            journal_name: DEFAULT_JOURNAL_NAME.to_string(),
            marker_name: DEFAULT_MARKER_NAME.to_string(),
            kdf_info: DEFAULT_KDF_INFO.to_string(),
        }
    }

    pub fn key_path(&self) -> PathBuf {
        self.dir.join(&self.key_name)
    }

    pub fn vault_path(&self) -> PathBuf {
        self.dir.join(&self.vault_name)
    }

    pub fn journal_path(&self) -> PathBuf {
        self.dir.join(&self.journal_name)
    }

    pub fn marker_path(&self) -> PathBuf {
        self.dir.join(&self.marker_name)
    }
}

/// One account that used to live in the OS keychain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyItem {
    pub service: String,
    pub account: String,
    /// Key written into the new vault. Defaults to `account`.
    pub store_key: String,
}

impl LegacyItem {
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        let account = account.into();
        Self {
            service: service.into(),
            store_key: account.clone(),
            account,
        }
    }
}

/// Read and delete access to the previous secret backend.
///
/// `read` must not include the secret in an error string. `delete` is called
/// only after the encrypted file has been verified.
pub trait LegacySecretSource {
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError>;
    fn delete(&self, service: &str, account: &str) -> Result<(), SecretStoreError>;
}

/// Legacy backend that fails if it is ever called. Used when no accounts are
/// registered, so a bug cannot silently reach the OS keychain.
pub struct RejectingLegacy;

impl LegacySecretSource for RejectingLegacy {
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError> {
        Err(SecretStoreError::Legacy(format!(
            "no legacy keychain is registered ({service} / {account})"
        )))
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::Legacy(format!(
            "no legacy keychain is registered ({service} / {account})"
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub already_migrated: bool,
    pub legacy_reads: usize,
    pub imported: usize,
    pub absent: usize,
    pub kept_existing: usize,
    pub deleted: usize,
    pub delete_failures: usize,
}

impl MigrationReport {
    fn fresh() -> Self {
        Self {
            already_migrated: false,
            legacy_reads: 0,
            imported: 0,
            absent: 0,
            kept_existing: 0,
            deleted: 0,
            delete_failures: 0,
        }
    }
}

#[derive(Debug)]
pub enum SecretStoreError {
    Io(io::Error),
    Crypto,
    Corrupt,
    VerifyFailed,
    EmptyAccount,
    RefusingSymlink,
    MissingSeed,
    NotOwner,
    Legacy(String),
}

impl SecretStoreError {
    pub fn legacy(err: impl std::fmt::Display) -> Self {
        Self::Legacy(err.to_string())
    }
}

impl std::fmt::Display for SecretStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "secret store io: {err}"),
            Self::Crypto => write!(f, "secret store crypto failed"),
            Self::Corrupt => write!(f, "secret store file is corrupt"),
            Self::VerifyFailed => {
                write!(f, "secret store could not read back the value just written")
            }
            Self::EmptyAccount => write!(f, "secret store account is empty"),
            Self::RefusingSymlink => write!(f, "secret store refuses to follow a symlink"),
            Self::MissingSeed => write!(
                f,
                "secret store seed is missing while the vault exists (restore secret.key, or delete secrets.vault after confirming you want to reset the store)"
            ),
            Self::NotOwner => {
                write!(f, "secret store directory is not owned by the current user")
            }
            Self::Legacy(msg) => write!(f, "legacy secret source: {msg}"),
        }
    }
}

impl std::error::Error for SecretStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for SecretStoreError {
    fn from(err: io::Error) -> Self {
        if is_symlink_error(&err) {
            Self::RefusingSymlink
        } else {
            Self::Io(err)
        }
    }
}

#[derive(Serialize, Deserialize)]
struct VaultFile {
    v: u32,
    entries: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct JournalFile {
    done: BTreeSet<String>,
}

/// Encrypted secret map in [`StorePaths`].
pub struct FileSecretStore {
    paths: StorePaths,
    key: [u8; KEY_LEN],
    #[cfg(test)]
    fail_readback: std::cell::Cell<bool>,
}

impl Drop for FileSecretStore {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

impl FileSecretStore {
    pub fn open(paths: StorePaths) -> Result<Self, SecretStoreError> {
        ensure_private_dir(&paths.dir)?;
        let mut ikm = load_or_create_seed(&paths)?;
        let key = derive_key(&ikm, paths.kdf_info.as_bytes())?;
        ikm.zeroize();
        Ok(Self {
            paths,
            key,
            #[cfg(test)]
            fail_readback: std::cell::Cell::new(false),
        })
    }

    pub fn get(&self, account: &str) -> Result<Option<String>, SecretStoreError> {
        reject_empty(account)?;
        let _lock = lock_store(&self.paths.dir)?;
        let entries = self.read_entries()?;
        Ok(entries.get(account).cloned())
    }

    pub fn set(&mut self, account: &str, secret: &str) -> Result<(), SecretStoreError> {
        reject_empty(account)?;
        let _lock = lock_store(&self.paths.dir)?;
        let mut entries = self.read_entries()?;
        entries.insert(account.to_string(), secret.to_string());
        self.write_entries(&entries)
    }

    pub fn delete(&mut self, account: &str) -> Result<(), SecretStoreError> {
        reject_empty(account)?;
        let _lock = lock_store(&self.paths.dir)?;
        let mut entries = self.read_entries()?;
        entries.remove(account);
        self.write_entries(&entries)
    }

    /// Import `items` from `legacy`, then never call it again for those accounts.
    pub fn migrate_from_legacy(
        &mut self,
        legacy: &dyn LegacySecretSource,
        items: &[LegacyItem],
    ) -> Result<MigrationReport, SecretStoreError> {
        if items.is_empty() {
            return Ok(MigrationReport::fresh());
        }
        let _lock = lock_store(&self.paths.dir)?;
        if regular_file_exists(&self.paths.marker_path())? {
            let mut report = MigrationReport::fresh();
            report.already_migrated = true;
            return Ok(report);
        }

        let mut report = MigrationReport::fresh();
        let mut journal = self.read_journal()?;
        let mut entries = self.read_entries()?;

        for item in items {
            reject_empty(&item.store_key)?;
            reject_empty(&item.account)?;
            if journal.done.contains(&item.store_key) {
                continue;
            }
            if entries.contains_key(&item.store_key) {
                // A value is already on disk. Do not read or delete the legacy
                // item: the vault copy might not be that legacy secret, and
                // deleting it would be irreversible.
                journal.done.insert(item.store_key.clone());
                self.write_journal(&journal)?;
                report.kept_existing += 1;
                continue;
            }

            let value = legacy.read(&item.service, &item.account)?;
            report.legacy_reads += 1;
            match value {
                Some(secret) => {
                    let mut updated = entries.clone();
                    updated.insert(item.store_key.clone(), secret.clone());
                    if let Err(err) = self.persist_and_verify(&updated, &item.store_key, &secret) {
                        // Verify failed: put the previous vault back if we can,
                        // and do not delete the legacy item.
                        let _ = self.write_entries(&entries);
                        return Err(err);
                    }
                    entries = updated;
                    journal.done.insert(item.store_key.clone());
                    self.write_journal(&journal)?;
                    report.imported += 1;
                    match legacy.delete(&item.service, &item.account) {
                        Ok(()) => report.deleted += 1,
                        Err(_) => report.delete_failures += 1,
                    }
                }
                None => {
                    journal.done.insert(item.store_key.clone());
                    self.write_journal(&journal)?;
                    report.absent += 1;
                }
            }
        }

        self.write_marker()?;
        Ok(report)
    }

    #[cfg(test)]
    fn fail_next_readback(&self) {
        self.fail_readback.set(true);
    }

    fn persist_and_verify(
        &self,
        entries: &BTreeMap<String, String>,
        account: &str,
        expected: &str,
    ) -> Result<(), SecretStoreError> {
        self.write_entries(entries)?;
        #[cfg(test)]
        if self.fail_readback.replace(false) {
            return Err(SecretStoreError::VerifyFailed);
        }
        match self.read_entries()?.get(account).map(String::as_str) {
            Some(got) if got == expected => Ok(()),
            _ => Err(SecretStoreError::VerifyFailed),
        }
    }

    fn read_entries(&self) -> Result<BTreeMap<String, String>, SecretStoreError> {
        let Some(bytes) = read_regular_file(&self.paths.vault_path())? else {
            return Ok(BTreeMap::new());
        };
        let mut plain = decrypt(&self.key, &bytes)?;
        let vault: VaultFile = match serde_json::from_slice(&plain) {
            Ok(vault) => vault,
            Err(_) => {
                plain.zeroize();
                return Err(SecretStoreError::Corrupt);
            }
        };
        plain.zeroize();
        if vault.v != VAULT_VERSION {
            return Err(SecretStoreError::Corrupt);
        }
        Ok(vault.entries)
    }

    fn write_entries(&self, entries: &BTreeMap<String, String>) -> Result<(), SecretStoreError> {
        let vault = VaultFile {
            v: VAULT_VERSION,
            entries: entries.clone(),
        };
        let mut plain = serde_json::to_vec(&vault).map_err(|_| SecretStoreError::Corrupt)?;
        let sealed = encrypt(&self.key, &plain)?;
        plain.zeroize();
        write_private_file(&self.paths.vault_path(), &sealed)
    }

    fn read_journal(&self) -> Result<JournalFile, SecretStoreError> {
        let Some(bytes) = read_regular_file(&self.paths.journal_path())? else {
            return Ok(JournalFile::default());
        };
        serde_json::from_slice(&bytes).map_err(|_| SecretStoreError::Corrupt)
    }

    fn write_journal(&self, journal: &JournalFile) -> Result<(), SecretStoreError> {
        let bytes = serde_json::to_vec(journal).map_err(|_| SecretStoreError::Corrupt)?;
        write_private_file(&self.paths.journal_path(), &bytes)
    }

    fn write_marker(&self) -> Result<(), SecretStoreError> {
        write_private_file(&self.paths.marker_path(), b"ok\n")
    }
}

/// Startup hook for the desktop shell.
///
/// tongyun-dev has no legacy keychain accounts, so this returns without
/// creating files and without calling [`LegacySecretSource`].
pub fn migrate_app_secrets(dir: &Path) -> Result<MigrationReport, SecretStoreError> {
    migrate_in_dir(&StorePaths::new(dir), &RejectingLegacy, &[])
}

pub fn migrate_in_dir(
    paths: &StorePaths,
    legacy: &dyn LegacySecretSource,
    items: &[LegacyItem],
) -> Result<MigrationReport, SecretStoreError> {
    if items.is_empty() {
        return Ok(MigrationReport::fresh());
    }
    if regular_file_exists(&paths.marker_path())? {
        let mut report = MigrationReport::fresh();
        report.already_migrated = true;
        return Ok(report);
    }
    let mut store = FileSecretStore::open(paths.clone())?;
    store.migrate_from_legacy(legacy, items)
}

fn reject_empty(account: &str) -> Result<(), SecretStoreError> {
    if account.is_empty() {
        Err(SecretStoreError::EmptyAccount)
    } else {
        Ok(())
    }
}

fn derive_key(ikm: &[u8; KEY_LEN], info: &[u8]) -> Result<[u8; KEY_LEN], SecretStoreError> {
    let hk = Hkdf::<Sha256>::new(Some(HKDF_SALT), ikm);
    let mut okm = [0u8; KEY_LEN];
    hk.expand(info, &mut okm)
        .map_err(|_| SecretStoreError::Crypto)?;
    Ok(okm)
}

fn load_or_create_seed(paths: &StorePaths) -> Result<[u8; KEY_LEN], SecretStoreError> {
    let _lock = lock_store(&paths.dir)?;
    if let Some(seed) = read_seed(&paths.key_path())? {
        return Ok(seed);
    }
    // A vault (or a symlink where the vault should be) with no seed must not
    // grow a fresh seed. The ciphertext would be unreadable, and a symlink
    // must not be replaced.
    match regular_file_exists(&paths.vault_path()) {
        Ok(true) => return Err(SecretStoreError::MissingSeed),
        Ok(false) => {}
        Err(err) => return Err(err),
    }
    let mut seed = [0u8; KEY_LEN];
    OsRng.fill_bytes(&mut seed);
    match create_seed_exclusive(&paths.key_path(), &seed) {
        Ok(()) => Ok(seed),
        Err(SecretStoreError::Io(err)) if err.kind() == io::ErrorKind::AlreadyExists => {
            seed.zeroize();
            match read_seed(&paths.key_path())? {
                Some(winner) => Ok(winner),
                None => Err(SecretStoreError::MissingSeed),
            }
        }
        Err(err) => {
            seed.zeroize();
            Err(err)
        }
    }
}

fn encrypt(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Result<Vec<u8>, SecretStoreError> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| SecretStoreError::Crypto)?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            nonce,
            chacha20poly1305::aead::Payload {
                msg: plaintext,
                aad: MAGIC,
            },
        )
        .map_err(|_| SecretStoreError::Crypto)?;
    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt(key: &[u8; KEY_LEN], blob: &[u8]) -> Result<Vec<u8>, SecretStoreError> {
    if blob.len() < MAGIC.len() + NONCE_LEN + 16 || !blob.starts_with(MAGIC) {
        return Err(SecretStoreError::Corrupt);
    }
    let nonce = XNonce::from_slice(&blob[MAGIC.len()..MAGIC.len() + NONCE_LEN]);
    let ciphertext = &blob[MAGIC.len() + NONCE_LEN..];
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| SecretStoreError::Crypto)?;
    cipher
        .decrypt(
            nonce,
            chacha20poly1305::aead::Payload {
                msg: ciphertext,
                aad: MAGIC,
            },
        )
        .map_err(|_| SecretStoreError::Crypto)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), SecretStoreError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            ensure_private_dir(parent)?;
        }
    }
    // Refuse to replace a symlink. rename would swap the directory entry, but
    // the caller asked us not to touch a link planted at this path.
    match regular_file_exists(path) {
        Ok(_) => {}
        Err(err) => return Err(err),
    }
    for _ in 0..8 {
        let tmp = temp_path_for(path);
        match open_create_new_nofollow(&tmp) {
            Ok(mut file) => {
                file.write_all(bytes)?;
                file.sync_all()?;
                set_file_mode(&file, 0o600)?;
                drop(file);
                if let Err(err) = fs::rename(&tmp, path) {
                    let _ = fs::remove_file(&tmp);
                    return Err(err.into());
                }
                if let Ok(file) = open_read_nofollow(path) {
                    set_file_mode(&file, 0o600)?;
                }
                sync_parent(path)?;
                return Ok(());
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "temp file collision").into())
}

/// Directory for the seed and vault: mode `0700`, not a symlink, owned by us.
///
/// **Ancestor directories**: Only `dir` itself is checked for symlinks; parent
/// directories such as `~/.config` are assumed to be trusted within the same
/// local user security boundary.
fn ensure_private_dir(dir: &Path) -> Result<(), SecretStoreError> {
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => return Err(SecretStoreError::RefusingSymlink),
        Ok(meta) if meta.is_dir() => {
            ensure_owner(&meta)?;
            restrict_dir(dir)?;
            return Ok(());
        }
        Ok(_) => return Err(SecretStoreError::Corrupt),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    if let Some(parent) = dir.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => return Err(SecretStoreError::RefusingSymlink),
        Ok(meta) if meta.is_dir() => {
            ensure_owner(&meta)?;
            restrict_dir(dir)?;
            return Ok(());
        }
        Ok(_) => return Err(SecretStoreError::Corrupt),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    #[allow(unused_mut)]
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(dir) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            let meta = fs::symlink_metadata(dir)?;
            if meta.file_type().is_symlink() {
                return Err(SecretStoreError::RefusingSymlink);
            }
            if !meta.is_dir() {
                return Err(SecretStoreError::Corrupt);
            }
            ensure_owner(&meta)?;
        }
        Err(err) => return Err(err.into()),
    }
    restrict_dir(dir)?;
    Ok(())
}

fn read_seed(path: &Path) -> Result<Option<[u8; KEY_LEN]>, SecretStoreError> {
    match read_regular_file(path)? {
        None => Ok(None),
        Some(mut buf) if buf.len() == KEY_LEN => {
            let mut seed = [0u8; KEY_LEN];
            seed.copy_from_slice(&buf);
            buf.zeroize();
            Ok(Some(seed))
        }
        Some(mut buf) => {
            buf.zeroize();
            Err(SecretStoreError::Corrupt)
        }
    }
}

fn create_seed_exclusive(path: &Path, seed: &[u8; KEY_LEN]) -> Result<(), SecretStoreError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            ensure_private_dir(parent)?;
        }
    }
    // Refuse to replace a symlink or an existing seed file.
    match regular_file_exists(path) {
        Ok(true) => {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "seed already exists").into())
        }
        Ok(false) => {}
        Err(err) => return Err(err),
    }
    for _ in 0..8 {
        let tmp = temp_path_for(path);
        match open_create_new_nofollow(&tmp) {
            Ok(mut file) => {
                file.write_all(seed)?;
                file.sync_all()?;
                set_file_mode(&file, 0o600)?;
                drop(file);

                #[cfg(unix)]
                {
                    match fs::hard_link(&tmp, path) {
                        Ok(()) => {
                            let _ = fs::remove_file(&tmp);
                        }
                        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                            let _ = fs::remove_file(&tmp);
                            return Err(err.into());
                        }
                        Err(_) => {
                            // Filesystem may not support hard links; fall back to rename
                            // while ensuring destination does not exist.
                            if regular_file_exists(path)? {
                                let _ = fs::remove_file(&tmp);
                                return Err(io::Error::new(
                                    io::ErrorKind::AlreadyExists,
                                    "seed already exists",
                                )
                                .into());
                            }
                            if let Err(err) = fs::rename(&tmp, path) {
                                let _ = fs::remove_file(&tmp);
                                return Err(err.into());
                            }
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    if regular_file_exists(path)? {
                        let _ = fs::remove_file(&tmp);
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "seed already exists",
                        )
                        .into());
                    }
                    if let Err(err) = fs::rename(&tmp, path) {
                        let _ = fs::remove_file(&tmp);
                        return Err(err.into());
                    }
                }

                if let Ok(file) = open_read_nofollow(path) {
                    set_file_mode(&file, 0o600)?;
                }
                sync_parent(path)?;
                return Ok(());
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "temp file collision").into())
}

fn read_regular_file(path: &Path) -> Result<Option<Vec<u8>>, SecretStoreError> {
    let mut file = match open_read_nofollow(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let meta = file.metadata()?;
    if !meta.file_type().is_file() {
        return Err(SecretStoreError::Corrupt);
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(Some(buf))
}

fn regular_file_exists(path: &Path) -> Result<bool, SecretStoreError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(SecretStoreError::RefusingSymlink),
        Ok(meta) if meta.is_file() => Ok(true),
        Ok(_) => Err(SecretStoreError::Corrupt),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

struct StoreLock {
    #[cfg(unix)]
    _file: File,
    #[cfg(not(unix))]
    _guard: std::sync::MutexGuard<'static, ()>,
}

/// Acquire an exclusive lock on the secret store directory.
///
/// # Non-reentrant / 不可嵌套
///
/// **Do not call `lock_store` while already holding a `StoreLock`.**
/// - On Unix, this opens `.secrets.lock` and applies `libc::flock(..., LOCK_EX)`.
///   Because each call opens a distinct file descriptor, acquiring the lock a
///   second time on the same thread or process will deadlock waiting for itself.
/// - On non-Unix platforms, the fallback uses a process-wide `std::sync::Mutex`,
///   which also deadlocks when locked recursively by the same thread.
///
/// # Porting note (tongyun-git / tongyun-api)
///
/// Keep internal read/write helpers (`read_entries`, `write_entries`,
/// `write_journal`, `write_marker`) lock-free. Acquire `lock_store` only at the
/// public API boundary (`get`, `set`, `delete`, `migrate_from_legacy`,
/// `load_or_create_seed`). Never invoke another public method from within an
/// operation that already holds `StoreLock`.
fn lock_store(dir: &Path) -> Result<StoreLock, SecretStoreError> {
    ensure_private_dir(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::io::AsRawFd;
        let path = dir.join(LOCK_NAME);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        options.mode(0o600);
        options.custom_flags(libc::O_NOFOLLOW);
        let file = options.open(&path)?;
        set_file_mode(&file, 0o600)?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if rc != 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(StoreLock { _file: file })
    }
    #[cfg(not(unix))]
    {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _ = dir;
        Ok(StoreLock {
            _guard: LOCK.lock().unwrap_or_else(|err| err.into_inner()),
        })
    }
}

fn temp_path_for(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "secret".to_string());
    let mut nonce = [0u8; 4];
    OsRng.fill_bytes(&mut nonce);
    let tmp_name = format!(".{name}.{}.{}.tmp", std::process::id(), hex4(nonce));
    match path.parent() {
        Some(parent) => parent.join(tmp_name),
        None => PathBuf::from(tmp_name),
    }
}

fn hex4(bytes: [u8; 4]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(8);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn open_create_new_nofollow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn open_read_nofollow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn set_file_mode(file: &File, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (file, mode);
    }
    Ok(())
}

fn restrict_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = OpenOptions::new();
        options.read(true);
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        let file = options.open(dir)?;
        set_file_mode(&file, 0o700)?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

fn ensure_owner(meta: &fs::Metadata) -> Result<(), SecretStoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = unsafe { libc::geteuid() };
        if meta.uid() != uid {
            return Err(SecretStoreError::NotOwner);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
    }
    Ok(())
}

fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(parent) = path.parent() {
            if parent.as_os_str().is_empty() {
                return Ok(());
            }
            let mut options = OpenOptions::new();
            options.read(true);
            options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
            let dir = options.open(parent)?;
            dir.sync_all()?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn is_symlink_error(err: &io::Error) -> bool {
    #[cfg(unix)]
    {
        err.raw_os_error() == Some(libc::ELOOP)
    }
    #[cfg(not(unix))]
    {
        let _ = err;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir =
                std::env::temp_dir().join(format!("tysec-{}-{nanos}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }

        fn paths(&self) -> StorePaths {
            StorePaths::new(&self.0)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct MemLegacy {
        items: Mutex<BTreeMap<(String, String), String>>,
        reads: Mutex<Vec<(String, String)>>,
        deletes: Mutex<Vec<(String, String)>>,
        fail_read: Mutex<bool>,
        fail_delete: Mutex<bool>,
        fail_on: Mutex<Option<String>>,
    }

    impl MemLegacy {
        fn with(items: Vec<(&str, &str, &str)>) -> Self {
            let mut map = BTreeMap::new();
            for (service, account, value) in items {
                map.insert(
                    (service.to_string(), account.to_string()),
                    value.to_string(),
                );
            }
            Self {
                items: Mutex::new(map),
                reads: Mutex::new(Vec::new()),
                deletes: Mutex::new(Vec::new()),
                fail_read: Mutex::new(false),
                fail_delete: Mutex::new(false),
                fail_on: Mutex::new(None),
            }
        }

        fn read_count(&self) -> usize {
            self.reads.lock().expect("reads").len()
        }

        fn delete_count(&self) -> usize {
            self.deletes.lock().expect("deletes").len()
        }
    }

    impl LegacySecretSource for MemLegacy {
        fn read(&self, service: &str, account: &str) -> Result<Option<String>, SecretStoreError> {
            self.reads
                .lock()
                .expect("reads")
                .push((service.to_string(), account.to_string()));
            if *self.fail_read.lock().expect("fail_read")
                || self.fail_on.lock().expect("fail_on").as_deref() == Some(account)
            {
                return Err(SecretStoreError::legacy("legacy read failed"));
            }
            Ok(self
                .items
                .lock()
                .expect("items")
                .get(&(service.to_string(), account.to_string()))
                .cloned())
        }

        fn delete(&self, service: &str, account: &str) -> Result<(), SecretStoreError> {
            self.deletes
                .lock()
                .expect("deletes")
                .push((service.to_string(), account.to_string()));
            if *self.fail_delete.lock().expect("fail_delete") {
                return Err(SecretStoreError::legacy("legacy delete failed"));
            }
            self.items
                .lock()
                .expect("items")
                .remove(&(service.to_string(), account.to_string()));
            Ok(())
        }
    }

    struct PanicLegacy;

    impl LegacySecretSource for PanicLegacy {
        fn read(&self, _: &str, _: &str) -> Result<Option<String>, SecretStoreError> {
            panic!("legacy read must not run");
        }

        fn delete(&self, _: &str, _: &str) -> Result<(), SecretStoreError> {
            panic!("legacy delete must not run");
        }
    }

    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() || haystack.len() < needle.len() {
            return false;
        }
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[test]
    fn roundtrip_set_get_delete_and_file_mode() {
        let dir = TempDir::new();
        let mut store = FileSecretStore::open(dir.paths()).unwrap();
        assert_eq!(store.get("demo-account").unwrap(), None);
        store.set("demo-account", "fixture-value").unwrap();
        store.set("other", "fixture-值").unwrap();
        assert_eq!(
            store.get("demo-account").unwrap().as_deref(),
            Some("fixture-value")
        );
        assert_eq!(store.get("other").unwrap().as_deref(), Some("fixture-值"));
        store.delete("demo-account").unwrap();
        assert_eq!(store.get("demo-account").unwrap(), None);
        assert_eq!(store.get("other").unwrap().as_deref(), Some("fixture-值"));

        let vault = fs::read(dir.paths().vault_path()).unwrap();
        assert!(!contains_bytes(&vault, b"fixture-value"));
        assert!(!contains_bytes(&vault, "fixture-值".as_bytes()));
        #[cfg(unix)]
        {
            assert_eq!(mode_of(&dir.paths().key_path()), 0o600);
            assert_eq!(mode_of(&dir.paths().vault_path()), 0o600);
        }
    }

    #[test]
    fn three_setting_writes_do_not_touch_legacy() {
        let dir = TempDir::new();
        let paths = dir.paths();
        let legacy = MemLegacy::with(vec![("com.example.app", "settings", "old-fixture")]);
        let items = vec![LegacyItem::new("com.example.app", "settings")];
        let first = migrate_in_dir(&paths, &legacy, &items).unwrap();
        assert_eq!(first.imported, 1);
        assert_eq!(first.legacy_reads, 1);
        assert_eq!(legacy.read_count(), 1);
        assert_eq!(legacy.delete_count(), 1);

        let mut store = FileSecretStore::open(paths).unwrap();
        store.set("settings", "one").unwrap();
        store.set("settings", "two").unwrap();
        store.set("settings", "three").unwrap();
        assert_eq!(store.get("settings").unwrap().as_deref(), Some("three"));
        assert_eq!(
            legacy.read_count(),
            1,
            "three saves must not re-read keychain"
        );
        assert_eq!(legacy.delete_count(), 1);

        let again = migrate_in_dir(&dir.paths(), &PanicLegacy, &items).unwrap();
        assert!(again.already_migrated);
    }

    #[test]
    fn migration_imports_once_then_skips_legacy() {
        let dir = TempDir::new();
        let paths = dir.paths();
        let legacy = MemLegacy::with(vec![
            ("com.example.app", "alpha", "fixture-a"),
            ("com.example.app", "beta", "fixture-b"),
        ]);
        let items = vec![
            LegacyItem::new("com.example.app", "alpha"),
            LegacyItem::new("com.example.app", "beta"),
        ];
        let report = migrate_in_dir(&paths, &legacy, &items).unwrap();
        assert_eq!(report.imported, 2);
        assert_eq!(report.deleted, 2);
        assert_eq!(report.legacy_reads, 2);
        assert!(legacy.items.lock().unwrap().is_empty());

        let store = FileSecretStore::open(paths.clone()).unwrap();
        assert_eq!(store.get("alpha").unwrap().as_deref(), Some("fixture-a"));
        assert_eq!(store.get("beta").unwrap().as_deref(), Some("fixture-b"));

        let second = migrate_in_dir(&paths, &PanicLegacy, &items).unwrap();
        assert!(second.already_migrated);
        assert_eq!(second.legacy_reads, 0);
        #[cfg(unix)]
        {
            assert_eq!(mode_of(&paths.marker_path()), 0o600);
            assert_eq!(mode_of(&paths.journal_path()), 0o600);
        }
    }

    #[test]
    fn failed_legacy_read_deletes_nothing_and_can_retry() {
        let dir = TempDir::new();
        let paths = dir.paths();
        let legacy = MemLegacy::with(vec![("com.example.app", "alpha", "fixture-a")]);
        *legacy.fail_read.lock().unwrap() = true;
        let items = vec![LegacyItem::new("com.example.app", "alpha")];
        let err = migrate_in_dir(&paths, &legacy, &items).unwrap_err();
        assert!(err.to_string().contains("legacy read failed"));
        assert!(!err.to_string().contains("fixture-a"));
        assert_eq!(legacy.delete_count(), 0);
        assert!(legacy
            .items
            .lock()
            .unwrap()
            .contains_key(&("com.example.app".into(), "alpha".into())));
        assert!(!paths.marker_path().exists());

        *legacy.fail_read.lock().unwrap() = false;
        let report = migrate_in_dir(&paths, &legacy, &items).unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(report.deleted, 1);
        let store = FileSecretStore::open(paths).unwrap();
        assert_eq!(store.get("alpha").unwrap().as_deref(), Some("fixture-a"));
    }

    #[test]
    fn verify_failure_rolls_back_and_does_not_delete_legacy() {
        let dir = TempDir::new();
        let paths = dir.paths();
        let mut store = FileSecretStore::open(paths.clone()).unwrap();
        store.fail_next_readback();
        let legacy = MemLegacy::with(vec![("com.example.app", "alpha", "fixture-a")]);
        let items = vec![LegacyItem::new("com.example.app", "alpha")];
        let err = store.migrate_from_legacy(&legacy, &items).unwrap_err();
        assert!(matches!(err, SecretStoreError::VerifyFailed));
        assert_eq!(legacy.delete_count(), 0);
        assert_eq!(legacy.read_count(), 1);
        assert!(!paths.marker_path().exists());
        drop(store);
        let store = FileSecretStore::open(paths).unwrap();
        assert_eq!(store.get("alpha").unwrap(), None);
        assert!(legacy
            .items
            .lock()
            .unwrap()
            .contains_key(&("com.example.app".into(), "alpha".into())));
    }

    #[test]
    fn delete_failure_keeps_file_and_does_not_read_again() {
        let dir = TempDir::new();
        let paths = dir.paths();
        let legacy = MemLegacy::with(vec![("com.example.app", "alpha", "fixture-a")]);
        *legacy.fail_delete.lock().unwrap() = true;
        let items = vec![LegacyItem::new("com.example.app", "alpha")];
        let report = migrate_in_dir(&paths, &legacy, &items).unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(report.deleted, 0);
        assert_eq!(report.delete_failures, 1);
        assert_eq!(legacy.read_count(), 1);
        assert!(legacy
            .items
            .lock()
            .unwrap()
            .contains_key(&("com.example.app".into(), "alpha".into())));

        let store = FileSecretStore::open(paths.clone()).unwrap();
        assert_eq!(store.get("alpha").unwrap().as_deref(), Some("fixture-a"));
        drop(store);

        let second = migrate_in_dir(&paths, &PanicLegacy, &items).unwrap();
        assert!(second.already_migrated);
        assert_eq!(legacy.read_count(), 1);
    }

    #[test]
    fn existing_vault_value_is_kept_and_legacy_is_not_read_or_deleted() {
        let dir = TempDir::new();
        let paths = dir.paths();
        let mut store = FileSecretStore::open(paths.clone()).unwrap();
        store.set("alpha", "already-here").unwrap();
        drop(store);

        let legacy = MemLegacy::with(vec![("com.example.app", "alpha", "from-keychain")]);
        let items = vec![LegacyItem::new("com.example.app", "alpha")];
        let report = migrate_in_dir(&paths, &legacy, &items).unwrap();
        assert_eq!(report.kept_existing, 1);
        assert_eq!(report.legacy_reads, 0);
        assert_eq!(legacy.read_count(), 0);
        assert_eq!(legacy.delete_count(), 0);
        let store = FileSecretStore::open(paths).unwrap();
        assert_eq!(store.get("alpha").unwrap().as_deref(), Some("already-here"));
    }

    #[test]
    fn absent_legacy_item_is_not_deleted_and_is_not_reread() {
        let dir = TempDir::new();
        let paths = dir.paths();
        let legacy = MemLegacy::with(vec![]);
        let items = vec![LegacyItem::new("com.example.app", "missing")];
        let report = migrate_in_dir(&paths, &legacy, &items).unwrap();
        assert_eq!(report.absent, 1);
        assert_eq!(report.legacy_reads, 1);
        assert_eq!(legacy.delete_count(), 0);
        let second = migrate_in_dir(&paths, &PanicLegacy, &items).unwrap();
        assert!(second.already_migrated);
    }

    #[test]
    fn partial_progress_does_not_reread_finished_accounts() {
        let dir = TempDir::new();
        let paths = dir.paths();
        let legacy = MemLegacy::with(vec![
            ("com.example.app", "alpha", "fixture-a"),
            ("com.example.app", "beta", "fixture-b"),
        ]);
        *legacy.fail_on.lock().unwrap() = Some("beta".to_string());
        let items = vec![
            LegacyItem::new("com.example.app", "alpha"),
            LegacyItem::new("com.example.app", "beta"),
        ];
        let err = migrate_in_dir(&paths, &legacy, &items).unwrap_err();
        assert!(err.to_string().contains("legacy read failed"));
        assert!(!paths.marker_path().exists());
        assert_eq!(legacy.delete_count(), 1);
        assert!(legacy
            .items
            .lock()
            .unwrap()
            .contains_key(&("com.example.app".into(), "beta".into())));
        assert!(!legacy
            .items
            .lock()
            .unwrap()
            .contains_key(&("com.example.app".into(), "alpha".into())));

        *legacy.fail_on.lock().unwrap() = None;
        let reads_before = legacy.read_count();
        let report = migrate_in_dir(&paths, &legacy, &items).unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(legacy.read_count(), reads_before + 1);
        let reads = legacy.reads.lock().unwrap().clone();
        assert_eq!(
            reads.last().map(|(s, a)| (s.as_str(), a.as_str())),
            Some(("com.example.app", "beta"))
        );
        assert!(!reads.iter().skip(reads_before).any(|(_, a)| a == "alpha"));
        let store = FileSecretStore::open(paths).unwrap();
        assert_eq!(store.get("alpha").unwrap().as_deref(), Some("fixture-a"));
        assert_eq!(store.get("beta").unwrap().as_deref(), Some("fixture-b"));
    }

    #[test]
    fn tamper_and_bad_seed_fail_closed_without_leaking_plaintext() {
        let dir = TempDir::new();
        let paths = dir.paths();
        let mut store = FileSecretStore::open(paths.clone()).unwrap();
        store.set("demo-account", "fixture-value").unwrap();
        drop(store);

        let mut vault = fs::read(paths.vault_path()).unwrap();
        let last = vault.len() - 1;
        vault[last] ^= 0xff;
        fs::write(paths.vault_path(), &vault).unwrap();
        let store = FileSecretStore::open(paths.clone()).unwrap();
        let err = store.get("demo-account").unwrap_err();
        assert!(matches!(err, SecretStoreError::Crypto));
        assert!(!err.to_string().contains("fixture-value"));
        drop(store);

        fs::write(paths.key_path(), b"short").unwrap();
        match FileSecretStore::open(paths) {
            Err(SecretStoreError::Corrupt) => {}
            Ok(_) => panic!("short seed must not open"),
            Err(other) => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn refuses_symlink_vault() {
        let dir = TempDir::new();
        let paths = dir.paths();
        #[allow(unused_mut)]
        let mut store = FileSecretStore::open(paths.clone()).unwrap();
        let outside = dir.0.join("outside.txt");
        fs::write(&outside, b"keep").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, paths.vault_path()).unwrap();
            let err = store.set("demo-account", "fixture-value").unwrap_err();
            assert!(matches!(err, SecretStoreError::RefusingSymlink));
            assert_eq!(fs::read(&outside).unwrap(), b"keep");
        }
        #[cfg(not(unix))]
        {
            let _ = (store, outside);
        }
    }

    #[test]
    fn empty_account_is_rejected() {
        let dir = TempDir::new();
        let mut store = FileSecretStore::open(dir.paths()).unwrap();
        assert!(matches!(
            store.set("", "x").unwrap_err(),
            SecretStoreError::EmptyAccount
        ));
    }

    #[test]
    fn app_hook_with_no_legacy_items_does_not_touch_disk() {
        let dir = TempDir::new();
        let report = migrate_app_secrets(&dir.0).unwrap();
        assert_eq!(report.legacy_reads, 0);
        assert!(!report.already_migrated);
        assert!(!dir.0.exists());
    }

    #[cfg(unix)]
    #[test]
    fn vault_dir_is_0700_even_if_it_started_world_accessible() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let paths = dir.paths();
        fs::create_dir_all(&paths.dir).unwrap();
        fs::set_permissions(&paths.dir, fs::Permissions::from_mode(0o755)).unwrap();
        let _store = FileSecretStore::open(paths.clone()).unwrap();
        assert_eq!(mode_of(&paths.dir), 0o700);
        assert_eq!(mode_of(&paths.dir.join(LOCK_NAME)), 0o600);
        assert_eq!(mode_of(&paths.key_path()), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_directory_and_does_not_write_into_the_target() {
        let dir = TempDir::new();
        let real = dir.0.join("real");
        fs::create_dir_all(&real).unwrap();
        let link = dir.0.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let opened = FileSecretStore::open(StorePaths::new(&link));
        assert!(matches!(opened, Err(SecretStoreError::RefusingSymlink)));
        assert!(fs::read_dir(&real).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_seed_and_journal() {
        let dir = TempDir::new();
        let paths = dir.paths();
        fs::create_dir_all(&paths.dir).unwrap();
        let outside_seed = dir.0.join("outside-seed");
        fs::write(&outside_seed, vec![7u8; KEY_LEN]).unwrap();
        std::os::unix::fs::symlink(&outside_seed, paths.key_path()).unwrap();
        let opened = FileSecretStore::open(paths.clone());
        assert!(matches!(opened, Err(SecretStoreError::RefusingSymlink)));
        assert_eq!(fs::read(&outside_seed).unwrap(), vec![7u8; KEY_LEN]);
        fs::remove_file(paths.key_path()).unwrap();

        let mut store = FileSecretStore::open(paths.clone()).unwrap();
        store.set("demo-account", "fixture-value").unwrap();
        drop(store);
        let outside_journal = dir.0.join("outside-journal");
        fs::write(&outside_journal, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside_journal, paths.journal_path()).unwrap();
        let legacy = MemLegacy::with(vec![]);
        let err = migrate_in_dir(
            &paths,
            &legacy,
            &[LegacyItem::new("com.example.app", "missing")],
        )
        .unwrap_err();
        assert!(matches!(err, SecretStoreError::RefusingSymlink));
        assert_eq!(fs::read(&outside_journal).unwrap(), b"keep");
        assert_eq!(legacy.read_count(), 0);
    }

    #[test]
    fn missing_seed_with_vault_is_an_error_and_does_not_regenerate() {
        let dir = TempDir::new();
        let paths = dir.paths();
        let mut store = FileSecretStore::open(paths.clone()).unwrap();
        store.set("demo-account", "fixture-value").unwrap();
        drop(store);
        let vault_before = fs::read(paths.vault_path()).unwrap();
        fs::remove_file(paths.key_path()).unwrap();
        let opened = FileSecretStore::open(paths.clone());
        match &opened {
            Err(err) => {
                assert!(matches!(err, SecretStoreError::MissingSeed));
                assert!(!err.to_string().contains("fixture-value"));
            }
            Ok(_) => panic!("missing seed must not open"),
        }
        assert!(!paths.key_path().exists());
        assert_eq!(fs::read(paths.vault_path()).unwrap(), vault_before);
    }

    #[test]
    fn concurrent_first_creation_shares_one_seed() {
        let dir = TempDir::new();
        let paths = dir.paths();
        let barrier = Arc::new(Barrier::new(8));
        let mut handles = Vec::new();
        for i in 0..8 {
            let paths = paths.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                let mut store = FileSecretStore::open(paths).expect("open");
                store.set(&format!("acct-{i}"), "fixture").expect("set");
            }));
        }
        for handle in handles {
            handle.join().expect("thread");
        }
        let store = FileSecretStore::open(paths).unwrap();
        for i in 0..8 {
            assert_eq!(
                store.get(&format!("acct-{i}")).unwrap().as_deref(),
                Some("fixture")
            );
        }
    }

    #[test]
    fn reopened_store_uses_the_same_seed() {
        let dir = TempDir::new();
        let paths = dir.paths();
        let mut store = FileSecretStore::open(paths.clone()).unwrap();
        store.set("demo-account", "fixture-value").unwrap();
        drop(store);
        let store = FileSecretStore::open(paths).unwrap();
        assert_eq!(
            store.get("demo-account").unwrap().as_deref(),
            Some("fixture-value")
        );
    }

    #[test]
    fn missing_seed_display_message_contains_recovery_guidance() {
        let msg = SecretStoreError::MissingSeed.to_string();
        assert!(msg.contains("restore secret.key"));
        assert!(msg.contains("delete secrets.vault"));
    }
}
