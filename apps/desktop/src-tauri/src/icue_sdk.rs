//! Consented download of Corsair's iCUE SDK runtime library.
//!
//! The pinned release archive is fetched over HTTPS from GitHub, checked against a SHA-256
//! constant, and a single member (the 64-bit library) is extracted, checked against a second
//! constant and moved into place atomically. Nothing else from the archive is read or written,
//! and archive member names are never used to build file-system paths.

use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::redirect::{Attempt, Policy};
use sha2::{Digest, Sha256};

/// Version of the pinned SDK release.
pub const SDK_VERSION: &str = "4.0.84";
const ZIP_URL: &str =
    "https://github.com/CorsairOfficial/cue-sdk/releases/download/v4.0.84/iCUESDK_4.0.84.zip";
const ZIP_SHA256: &str = "eb4414cf505145f3e507dc839ad587bf7ca684d64be3d9a834f416a136736d5d";
const DLL_ENTRY: &str = "iCUESDK/redist/x64/iCUESDK.x64_2019.dll";
const DLL_SHA256: &str = "d72fd819b91fd1d3b0c3db2ea17a47dcfe3b38e26269aa967ecfee10d2e93884";
/// File name the outputs crate loads.
const DLL_FILE_NAME: &str = "iCUESDK.x64_2019.dll";

/// Upper bound for the downloaded archive (the pinned archive is about 340 KB).
const MAX_ZIP_BYTES: usize = 2 * 1024 * 1024;
/// Upper bound for the extracted library (the pinned library is about 500 KB).
const MAX_DLL_BYTES: u64 = 2 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REDIRECTS: usize = 5;
/// Hosts a request or redirect may target: GitHub and its release-asset hosts.
const ALLOWED_HOSTS: [&str; 4] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "github-releases.githubusercontent.com",
];

/// Expected digests and archive member of a release.
#[derive(Debug, Clone, Copy)]
struct Expected<'a> {
    zip_sha256: &'a str,
    entry: &'a str,
    dll_sha256: &'a str,
}

const PINNED: Expected<'static> = Expected {
    zip_sha256: ZIP_SHA256,
    entry: DLL_ENTRY,
    dll_sha256: DLL_SHA256,
};

/// Why the library could not be installed. `Display` is shown to the user.
#[derive(Debug)]
pub enum InstallError {
    Busy,
    Network(String),
    HttpStatus(u16),
    TooLarge,
    ArchiveChecksum,
    Archive(String),
    LibraryChecksum,
    Io(String),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("The library is already being downloaded."),
            Self::Network(detail) => write!(
                f,
                "Could not download the library from GitHub. Check your internet connection and try again. ({detail})"
            ),
            Self::HttpStatus(code) => {
                write!(
                    f,
                    "GitHub answered with HTTP status {code}. Try again later."
                )
            }
            Self::TooLarge => {
                f.write_str("The download was larger than expected and was discarded.")
            }
            Self::ArchiveChecksum | Self::LibraryChecksum => f.write_str(
                "The downloaded file did not match the expected checksum and was discarded.",
            ),
            Self::Archive(detail) => {
                write!(f, "The downloaded archive could not be read. ({detail})")
            }
            Self::Io(detail) => write!(f, "Could not save the library. ({detail})"),
        }
    }
}

impl std::error::Error for InstallError {}

/// Result of a successful install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    /// The verified library was written to disk.
    Written,
    /// An identical, verified library was already in place and was left untouched.
    AlreadyPresent,
}

/// Only one download runs at a time.
static IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct ProgressGuard;

impl ProgressGuard {
    fn acquire() -> Result<Self, InstallError> {
        IN_PROGRESS
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| InstallError::Busy)
    }
}

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        IN_PROGRESS.store(false, Ordering::Release);
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// True for `https` URLs whose host is one of [`ALLOWED_HOSTS`].
fn is_allowed_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| ALLOWED_HOSTS.contains(&host))
}

fn redirect_policy() -> Policy {
    Policy::custom(|attempt: Attempt<'_>| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if is_allowed_url(attempt.url()) {
            attempt.follow()
        } else {
            attempt.error("redirect to a host outside GitHub")
        }
    })
}

fn ensure_tls_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // Fails only when another provider was installed concurrently, which is equally fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

async fn download_archive(url: &str) -> Result<Vec<u8>, InstallError> {
    let network = |e: reqwest::Error| InstallError::Network(e.without_url().to_string());
    let parsed = reqwest::Url::parse(url).map_err(|e| InstallError::Network(e.to_string()))?;
    if !is_allowed_url(&parsed) {
        return Err(InstallError::Network(
            "address is not a GitHub URL".to_owned(),
        ));
    }
    ensure_tls_provider();
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(redirect_policy())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .user_agent(concat!("Glowdrift/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(network)?;
    let mut response = client.get(parsed).send().await.map_err(network)?;
    if !response.status().is_success() {
        return Err(InstallError::HttpStatus(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_ZIP_BYTES as u64)
    {
        return Err(InstallError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if body.len() + chunk.len() > MAX_ZIP_BYTES {
            return Err(InstallError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Verifies the archive digest, extracts the one expected member and verifies its digest.
fn extract_library(zip_bytes: &[u8], expected: &Expected<'_>) -> Result<Vec<u8>, InstallError> {
    if sha256_hex(zip_bytes) != expected.zip_sha256 {
        return Err(InstallError::ArchiveChecksum);
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
        .map_err(|e| InstallError::Archive(e.to_string()))?;
    let entry = archive
        .by_name(expected.entry)
        .map_err(|e| InstallError::Archive(e.to_string()))?;
    if !entry.is_file() || entry.size() > MAX_DLL_BYTES {
        return Err(InstallError::Archive("unexpected library entry".to_owned()));
    }
    let mut library = Vec::new();
    entry
        .take(MAX_DLL_BYTES + 1)
        .read_to_end(&mut library)
        .map_err(|e| InstallError::Archive(e.to_string()))?;
    if library.len() as u64 > MAX_DLL_BYTES {
        return Err(InstallError::TooLarge);
    }
    if sha256_hex(&library) != expected.dll_sha256 {
        return Err(InstallError::LibraryChecksum);
    }
    Ok(library)
}

fn file_has_digest(path: &Path, digest: &str) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.len() > MAX_DLL_BYTES {
        return false;
    }
    fs::read(path).is_ok_and(|bytes| sha256_hex(&bytes) == digest)
}

/// Writes `library` as `<dir>/<DLL_FILE_NAME>` through a temporary file and a rename, unless a
/// file with the expected digest is already there.
fn place_library(dir: &Path, library: &[u8], digest: &str) -> Result<Installed, InstallError> {
    let io = |e: std::io::Error| InstallError::Io(e.to_string());
    fs::create_dir_all(dir).map_err(io)?;
    let target = dir.join(DLL_FILE_NAME);
    if file_has_digest(&target, digest) {
        return Ok(Installed::AlreadyPresent);
    }
    let temp = dir.join(format!("{DLL_FILE_NAME}.{}.tmp", std::process::id()));
    let written = File::create(&temp)
        .and_then(|mut file| {
            file.write_all(library)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temp, &target));
    if let Err(e) = written {
        let _ = fs::remove_file(&temp);
        return Err(io(e));
    }
    Ok(Installed::Written)
}

/// Downloads, verifies and installs the pinned library into `dir`.
pub async fn install(dir: &Path) -> Result<Installed, InstallError> {
    let _guard = ProgressGuard::acquire()?;
    log::info!("downloading iCUE SDK {SDK_VERSION}");
    let archive = download_archive(ZIP_URL).await.inspect_err(|e| {
        log::warn!("iCUE SDK download failed: {e}");
    })?;
    let target = dir.to_path_buf();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let library = extract_library(&archive, &PINNED)?;
        place_library(&target, &library, PINNED.dll_sha256)
    })
    .await
    .map_err(|e| InstallError::Io(e.to_string()))?;
    match &outcome {
        Ok(installed) => log::info!("iCUE SDK {SDK_VERSION} verified ({installed:?})"),
        Err(e) => log::warn!("iCUE SDK install failed: {e}"),
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> reqwest::Url {
        reqwest::Url::parse(text).unwrap()
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    const NAME: &str = "pkg/redist/x64/lib.dll";

    fn expected_for<'a>(zip: &'a str, dll: &'a str) -> Expected<'a> {
        Expected {
            zip_sha256: zip,
            entry: NAME,
            dll_sha256: dll,
        }
    }

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "glowdrift-icue-sdk-test-{}-{tag}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn pinned_constants_are_lowercase_sha256_hex() {
        for digest in [ZIP_SHA256, DLL_SHA256] {
            assert_eq!(digest.len(), 64);
            assert!(
                digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            );
        }
        assert!(is_allowed_url(&url(ZIP_URL)));
        assert!(ZIP_URL.contains(SDK_VERSION));
    }

    #[test]
    fn sha256_matches_a_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn only_https_github_hosts_are_allowed() {
        assert!(is_allowed_url(&url("https://github.com/a/b")));
        assert!(is_allowed_url(&url(
            "https://release-assets.githubusercontent.com/x?y=1"
        )));
        assert!(!is_allowed_url(&url("http://github.com/a")));
        assert!(!is_allowed_url(&url("https://example.com/github.com")));
        assert!(!is_allowed_url(&url("https://github.com.evil.test/a")));
        assert!(!is_allowed_url(&url("https://evilgithubusercontent.com/a")));
        assert!(!is_allowed_url(&url("https://github.com@evil.test/a")));
    }

    #[test]
    fn extracts_and_verifies_the_expected_member() {
        let dll = b"library bytes".to_vec();
        let archive = zip_with(&[("pkg/readme.txt", b"hi"), (NAME, &dll)]);
        let zip_digest = sha256_hex(&archive);
        let dll_digest = sha256_hex(&dll);
        let out = extract_library(&archive, &expected_for(&zip_digest, &dll_digest)).unwrap();
        assert_eq!(out, dll);
    }

    #[test]
    fn rejects_archive_digest_mismatch() {
        let archive = zip_with(&[(NAME, b"x")]);
        let wrong = "0".repeat(64);
        assert!(matches!(
            extract_library(&archive, &expected_for(&wrong, &sha256_hex(b"x"))),
            Err(InstallError::ArchiveChecksum)
        ));
    }

    #[test]
    fn rejects_library_digest_mismatch() {
        let archive = zip_with(&[(NAME, b"tampered")]);
        let zip_digest = sha256_hex(&archive);
        assert!(matches!(
            extract_library(
                &archive,
                &expected_for(&zip_digest, &sha256_hex(b"original"))
            ),
            Err(InstallError::LibraryChecksum)
        ));
    }

    #[test]
    fn rejects_missing_member_and_garbage() {
        let archive = zip_with(&[("other.dll", b"x")]);
        let zip_digest = sha256_hex(&archive);
        assert!(matches!(
            extract_library(&archive, &expected_for(&zip_digest, &sha256_hex(b"x"))),
            Err(InstallError::Archive(_))
        ));
        let garbage = b"not a zip".to_vec();
        let zip_digest = sha256_hex(&garbage);
        assert!(matches!(
            extract_library(&garbage, &expected_for(&zip_digest, &sha256_hex(b"x"))),
            Err(InstallError::Archive(_))
        ));
    }

    #[test]
    fn member_names_with_traversal_never_reach_the_filesystem() {
        let dll = b"payload".to_vec();
        let archive = zip_with(&[("../../evil.dll", &dll)]);
        let zip_digest = sha256_hex(&archive);
        let result = extract_library(&archive, &expected_for(&zip_digest, &sha256_hex(&dll)));
        assert!(matches!(result, Err(InstallError::Archive(_))));
    }

    #[test]
    fn places_the_library_atomically_and_leaves_no_temp_file() {
        let tmp = TempDir::new("place");
        let dir = tmp.0.join("icue-sdk");
        let bytes = b"lib".to_vec();
        let digest = sha256_hex(&bytes);
        assert_eq!(
            place_library(&dir, &bytes, &digest).unwrap(),
            Installed::Written
        );
        assert_eq!(fs::read(dir.join(DLL_FILE_NAME)).unwrap(), bytes);
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(DLL_FILE_NAME)]);
    }

    #[test]
    fn valid_existing_library_is_not_rewritten_but_a_corrupt_one_is_replaced() {
        let tmp = TempDir::new("existing");
        let dir = &tmp.0;
        let bytes = b"lib".to_vec();
        let digest = sha256_hex(&bytes);
        place_library(dir, &bytes, &digest).unwrap();
        assert_eq!(
            place_library(dir, &bytes, &digest).unwrap(),
            Installed::AlreadyPresent
        );
        fs::write(dir.join(DLL_FILE_NAME), b"truncated").unwrap();
        assert_eq!(
            place_library(dir, &bytes, &digest).unwrap(),
            Installed::Written
        );
        assert_eq!(fs::read(dir.join(DLL_FILE_NAME)).unwrap(), bytes);
    }

    #[test]
    #[ignore = "downloads the pinned release from GitHub"]
    fn downloads_and_installs_the_pinned_release() {
        let tmp = TempDir::new("network");
        let dir = tmp.0.join("icue-sdk");
        let first = tauri::async_runtime::block_on(install(&dir)).unwrap();
        assert_eq!(first, Installed::Written);
        let bytes = fs::read(dir.join(DLL_FILE_NAME)).unwrap();
        assert_eq!(sha256_hex(&bytes), DLL_SHA256);
        let second = tauri::async_runtime::block_on(install(&dir)).unwrap();
        assert_eq!(second, Installed::AlreadyPresent);
    }

    #[test]
    fn only_one_install_runs_at_a_time() {
        let first = ProgressGuard::acquire().unwrap();
        assert!(matches!(ProgressGuard::acquire(), Err(InstallError::Busy)));
        drop(first);
        assert!(ProgressGuard::acquire().is_ok());
    }
}
