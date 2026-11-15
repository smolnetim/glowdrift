//! Encrypted storage for the Hue bridge credentials.
//!
//! The bytes handed to [`save`] are encrypted with DPAPI for the current Windows user and
//! written to `hue-bridge.bin` in the given directory through a temporary file and a rename.
//! The content is opaque to this module.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

/// File name of the encrypted credentials inside the storage directory.
pub const FILE_NAME: &str = "hue-bridge.bin";

/// Encrypts `plaintext` for the current user and replaces the stored file with it.
pub fn save(dir: &Path, plaintext: &[u8]) -> io::Result<()> {
    let encrypted = dpapi::protect(plaintext)?;
    fs::create_dir_all(dir)?;
    let target = dir.join(FILE_NAME);
    let temp = dir.join(format!("{FILE_NAME}.{}.tmp", std::process::id()));
    let written = File::create(&temp)
        .and_then(|mut file| {
            file.write_all(&encrypted)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temp, &target));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

/// Returns the stored plaintext, or `None` when nothing is stored. A file that cannot be
/// decrypted (corrupted, or written by another user) is an error.
pub fn load(dir: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(dir.join(FILE_NAME)) {
        Ok(encrypted) => dpapi::unprotect(&encrypted).map(Some),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Deletes the stored file. Succeeds when there is none.
pub fn clear(dir: &Path) -> io::Result<()> {
    match fs::remove_file(dir.join(FILE_NAME)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(windows)]
mod dpapi {
    use std::ffi::c_void;
    use std::io;

    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    use windows::core::w;

    /// Mixed into the key derivation; the same value is needed to decrypt.
    const ENTROPY: &[u8] = b"glowdrift.hue-bridge.v1";

    fn blob(data: &[u8]) -> io::Result<CRYPT_INTEGER_BLOB> {
        let len = u32::try_from(data.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "data too large"))?;
        Ok(CRYPT_INTEGER_BLOB {
            cbData: len,
            pbData: data.as_ptr().cast_mut(),
        })
    }

    /// Copies the DPAPI output buffer and releases it.
    ///
    /// # Safety
    /// `out` must hold a buffer allocated by a successful DPAPI call that nothing else frees.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: per the contract above, `pbData` points to `cbData` readable bytes.
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        // SAFETY: the buffer was allocated by DPAPI with LocalAlloc and is freed exactly once.
        unsafe { LocalFree(Some(HLOCAL(out.pbData.cast::<c_void>()))) };
        bytes
    }

    pub fn protect(plaintext: &[u8]) -> io::Result<Vec<u8>> {
        let input = blob(plaintext)?;
        let entropy = blob(ENTROPY)?;
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: `input` and `entropy` point to live slices for the duration of the call and
        // `output` is a valid destination.
        unsafe {
            CryptProtectData(
                &raw const input,
                w!("Glowdrift Hue bridge"),
                Some(&raw const entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        }
        .map_err(|e| io::Error::other(format!("DPAPI encryption failed: {e}")))?;
        // SAFETY: the call succeeded, so `output` holds a DPAPI-allocated buffer.
        Ok(unsafe { take(output) })
    }

    pub fn unprotect(encrypted: &[u8]) -> io::Result<Vec<u8>> {
        let input = blob(encrypted)?;
        let entropy = blob(ENTROPY)?;
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: `input` and `entropy` point to live slices for the duration of the call and
        // `output` is a valid destination.
        unsafe {
            CryptUnprotectData(
                &raw const input,
                None,
                Some(&raw const entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        }
        .map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("DPAPI decryption failed: {e}"),
            )
        })?;
        // SAFETY: the call succeeded, so `output` holds a DPAPI-allocated buffer.
        Ok(unsafe { take(output) })
    }
}

#[cfg(not(windows))]
mod dpapi {
    use std::io;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "credential encryption needs Windows",
        )
    }

    pub fn protect(_plaintext: &[u8]) -> io::Result<Vec<u8>> {
        Err(unsupported())
    }

    pub fn unprotect(_encrypted: &[u8]) -> io::Result<Vec<u8>> {
        Err(unsupported())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("glowdrift-hue-store-{}-{tag}", std::process::id()));
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
    fn round_trips_and_stores_ciphertext() {
        let tmp = TempDir::new("round-trip");
        let secret = br#"{"appKey":"super-secret-key"}"#;
        save(&tmp.0, secret).unwrap();
        assert_eq!(load(&tmp.0).unwrap().as_deref(), Some(&secret[..]));
        let on_disk = fs::read(tmp.0.join(FILE_NAME)).unwrap();
        assert!(!on_disk.windows(12).any(|w| w == b"super-secret"));
        let entries = fs::read_dir(&tmp.0).unwrap().count();
        assert_eq!(entries, 1, "temporary file left behind");
    }

    #[test]
    fn save_replaces_the_previous_value() {
        let tmp = TempDir::new("replace");
        save(&tmp.0, b"first").unwrap();
        save(&tmp.0, b"second").unwrap();
        assert_eq!(load(&tmp.0).unwrap().as_deref(), Some(&b"second"[..]));
    }

    #[test]
    fn empty_plaintext_round_trips() {
        let tmp = TempDir::new("empty");
        save(&tmp.0, b"").unwrap();
        assert_eq!(load(&tmp.0).unwrap().as_deref(), Some(&b""[..]));
    }

    #[test]
    fn missing_file_loads_as_none() {
        let tmp = TempDir::new("missing");
        assert_eq!(load(&tmp.0).unwrap(), None);
        fs::create_dir_all(&tmp.0).unwrap();
        assert_eq!(load(&tmp.0).unwrap(), None);
    }

    #[test]
    fn corrupted_file_is_an_error() {
        let tmp = TempDir::new("corrupt");
        fs::create_dir_all(&tmp.0).unwrap();
        fs::write(tmp.0.join(FILE_NAME), b"not a dpapi blob").unwrap();
        assert!(load(&tmp.0).is_err());
        save(&tmp.0, b"data").unwrap();
        let mut bytes = fs::read(tmp.0.join(FILE_NAME)).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        fs::write(tmp.0.join(FILE_NAME), bytes).unwrap();
        assert!(load(&tmp.0).is_err());
    }

    #[test]
    fn clear_removes_the_file_and_tolerates_a_missing_one() {
        let tmp = TempDir::new("clear");
        clear(&tmp.0).unwrap();
        save(&tmp.0, b"data").unwrap();
        clear(&tmp.0).unwrap();
        assert_eq!(load(&tmp.0).unwrap(), None);
        clear(&tmp.0).unwrap();
    }
}
