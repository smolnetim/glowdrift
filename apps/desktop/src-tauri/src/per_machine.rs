//! Per-machine install detection and the elevated launch of the update installer.
//!
//! A per-machine install lives under Program Files, so its installer needs administrator
//! rights. The installer is started with the `runas` verb before the app stops anything: a
//! declined permission prompt then leaves the running app, and the staged update, untouched.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Shown when the user declines the Windows permission prompt.
pub const PERMISSION_DECLINED: &str =
    "The update was not installed because administrator permission was not granted.";

/// Environment variables that name the Program Files folders.
const PROGRAM_FILES_VARS: [&str; 3] = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"];

/// True when the running executable is installed under Program Files.
pub fn running_app_is_per_machine() -> bool {
    let roots: Vec<PathBuf> = PROGRAM_FILES_VARS
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    std::env::current_exe().is_ok_and(|exe| is_under_any(&exe, &roots))
}

/// Lower-cased path with `/` as separator, without a `\\?\` prefix or trailing separators.
fn normalize(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.trim_end_matches('\\').to_lowercase()
}

/// True when the folder holding `exe` is one of `roots` or lies below one. Comparison is
/// case-insensitive and respects folder boundaries (`C:\Program Files Extra` is not below
/// `C:\Program Files`). Empty roots never match.
fn is_under_any(exe: &Path, roots: &[PathBuf]) -> bool {
    let Some(dir) = exe.parent().map(normalize) else {
        return false;
    };
    roots.iter().map(|root| normalize(root)).any(|root| {
        !root.is_empty()
            && (dir == root
                || dir
                    .strip_prefix(&root)
                    .is_some_and(|rest| rest.starts_with('\\')))
    })
}

/// Quotes one argument for the installer's command line. Arguments containing `/`, spaces or
/// tabs (or empty ones) are wrapped in quotes so the installer does not read them as options.
fn quote_argument(arg: &str) -> String {
    let quote = arg.is_empty() || arg.contains([' ', '\t', '/']);
    let mut out = String::with_capacity(arg.len() + 2);
    if quote {
        out.push('"');
    }
    let mut backslashes = 0_usize;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                out.extend(std::iter::repeat_n('\\', backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(c);
    }
    if quote {
        out.extend(std::iter::repeat_n('\\', backslashes));
        out.push('"');
    }
    out
}

/// Command line for the update installer: passive UI, update mode, and a restart of the app
/// with its current arguments afterwards. The installer detects the install mode itself.
pub fn installer_arguments(app_args: &[OsString]) -> OsString {
    let mut args = String::from("/P /UPDATE /R");
    if !app_args.is_empty() {
        args.push_str(" /ARGS");
        for arg in app_args {
            args.push(' ');
            args.push_str(&quote_argument(&arg.to_string_lossy()));
        }
    }
    OsString::from(args)
}

/// True when `bytes` start like a Windows executable.
fn is_executable_image(bytes: &[u8]) -> bool {
    bytes.starts_with(b"MZ")
}

/// Starts `installer` elevated, with `arguments`, and returns once the launch succeeded.
/// Declining the permission prompt yields [`PERMISSION_DECLINED`].
#[cfg(windows)]
pub fn launch_elevated(installer: &[u8], arguments: &OsStr) -> Result<(), String> {
    if !is_executable_image(installer) {
        return Err("The update file is not a valid installer.".to_owned());
    }
    let (path, lock) = windows_impl::stage_installer(installer)
        .map_err(|e| format!("The update could not be prepared for installation: {e}."))?;
    match windows_impl::shell_execute_runas(&path, arguments) {
        Ok(()) => {
            // Held until the process exits so the installer file stays unmodifiable.
            std::mem::forget(lock);
            Ok(())
        }
        Err(message) => {
            drop(lock);
            let _ = std::fs::remove_file(&path);
            Err(message)
        }
    }
}

/// Elevated launches exist on Windows only.
#[cfg(not(windows))]
pub fn launch_elevated(_installer: &[u8], _arguments: &OsStr) -> Result<(), String> {
    Err("Elevated updates are only supported on Windows.".to_owned())
}

#[cfg(windows)]
mod windows_impl {
    use std::fs::{File, OpenOptions};
    use std::hash::{BuildHasher, Hasher, RandomState};
    use std::io::{Read, Write};
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};

    use windows::Win32::Foundation::ERROR_CANCELLED;
    use windows::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
    use windows::core::{HRESULT, HSTRING, PCWSTR, w};

    use super::PERMISSION_DECLINED;

    /// `FILE_SHARE_READ`: other handles may read the file but not write to or delete it.
    const SHARE_READ_ONLY: u32 = 1;

    /// 64 bits from the per-process random seed of the standard hasher.
    fn random_u64() -> u64 {
        RandomState::new().build_hasher().finish()
    }

    /// Writes `bytes` to a new, uniquely named `.exe` in the user's temp folder, then reopens
    /// it read-only with writes and deletes denied and checks the content. The returned
    /// handle keeps the file locked until it is dropped. The file is removed on failure.
    pub(super) fn stage_installer(bytes: &[u8]) -> std::io::Result<(PathBuf, File)> {
        let name = format!(
            "glowdrift-update-{:016x}{:016x}.exe",
            random_u64(),
            random_u64()
        );
        let path = std::env::temp_dir().join(name);
        let result = write_and_lock(&path, bytes);
        if result.is_err() {
            let _ = std::fs::remove_file(&path);
        }
        result.map(|lock| (path, lock))
    }

    fn write_and_lock(path: &Path, bytes: &[u8]) -> std::io::Result<File> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(bytes)?;
        file.flush()?;
        drop(file);
        let mut lock = OpenOptions::new()
            .read(true)
            .share_mode(SHARE_READ_ONLY)
            .open(path)?;
        let mut written = Vec::with_capacity(bytes.len());
        lock.read_to_end(&mut written)?;
        if written != bytes {
            return Err(std::io::Error::other(
                "the installer file changed after it was written",
            ));
        }
        Ok(lock)
    }

    /// Launches `file` with the `runas` verb. A declined permission prompt maps to
    /// [`PERMISSION_DECLINED`].
    pub(super) fn shell_execute_runas(
        file: &Path,
        arguments: &std::ffi::OsStr,
    ) -> Result<(), String> {
        let file_w = HSTRING::from(file.as_os_str());
        let args_w = HSTRING::from(arguments);
        let dir_w = file
            .parent()
            .map_or_else(HSTRING::new, |dir| HSTRING::from(dir.as_os_str()));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: u32::try_from(size_of::<SHELLEXECUTEINFOW>()).unwrap_or(0),
            fMask: SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file_w.as_ptr()),
            lpParameters: PCWSTR(args_w.as_ptr()),
            lpDirectory: PCWSTR(dir_w.as_ptr()),
            nShow: SW_SHOW.0,
            ..Default::default()
        };
        // SAFETY: `info` is fully initialised and its string pointers outlive the call.
        let outcome = unsafe { ShellExecuteExW(&raw mut info) };
        if !info.hProcess.is_invalid() {
            // SAFETY: the handle was returned by the call above and is closed once.
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(info.hProcess) };
        }
        outcome.map_err(|e| {
            failure_message(
                e.code() == HRESULT::from_win32(ERROR_CANCELLED.0),
                &e.message(),
            )
        })
    }

    /// Message for a failed launch.
    pub(super) fn failure_message(declined: bool, detail: &str) -> String {
        if declined {
            PERMISSION_DECLINED.to_owned()
        } else {
            format!(
                "The update could not be installed: {}.",
                detail.trim_end_matches(['.', ' ', '\r', '\n'])
            )
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn declined_prompt_gets_the_friendly_message() {
            assert_eq!(
                failure_message(true, "The operation was canceled by the user."),
                PERMISSION_DECLINED
            );
        }

        #[test]
        fn other_failures_carry_the_system_text() {
            assert_eq!(
                failure_message(false, "Access is denied.\r\n"),
                "The update could not be installed: Access is denied."
            );
        }

        #[test]
        fn staged_installer_is_unique_locked_and_identical() {
            let bytes = b"MZ staged payload";
            let (first, lock_a) = stage_installer(bytes).unwrap();
            let (second, lock_b) = stage_installer(bytes).unwrap();
            assert_ne!(first, second);
            assert_eq!(first.extension().and_then(|e| e.to_str()), Some("exe"));
            assert_eq!(std::fs::read(&first).unwrap(), bytes);
            assert!(OpenOptions::new().write(true).open(&first).is_err());
            assert!(std::fs::remove_file(&first).is_err());
            drop((lock_a, lock_b));
            std::fs::remove_file(&first).unwrap();
            std::fs::remove_file(&second).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> Vec<PathBuf> {
        vec![
            PathBuf::from(r"C:\Program Files"),
            PathBuf::from(r"C:\Program Files (x86)"),
        ]
    }

    fn per_machine(exe: &str) -> bool {
        is_under_any(Path::new(exe), &roots())
    }

    #[test]
    fn exe_below_program_files_is_per_machine() {
        assert!(per_machine(r"C:\Program Files\T1M\Glowdrift\glowdrift.exe"));
        assert!(per_machine(
            r"C:\Program Files (x86)\T1M\Glowdrift\glowdrift.exe"
        ));
        assert!(per_machine(r"C:\Program Files\glowdrift.exe"));
    }

    #[test]
    fn comparison_ignores_case_and_separator_style() {
        assert!(per_machine(r"c:\PROGRAM FILES\t1m\glowdrift\Glowdrift.exe"));
        assert!(per_machine("C:/Program Files/T1M/Glowdrift/glowdrift.exe"));
        assert!(per_machine(
            r"\\?\C:\Program Files\T1M\Glowdrift\glowdrift.exe"
        ));
    }

    #[test]
    fn trailing_separators_on_roots_are_ignored() {
        let with_slash = [PathBuf::from(r"C:\Program Files\")];
        let exe = Path::new(r"C:\Program Files\T1M\glowdrift.exe");
        assert!(is_under_any(exe, &with_slash));
    }

    #[test]
    fn per_user_and_sibling_folders_are_not_per_machine() {
        assert!(!per_machine(
            r"C:\Users\tim\AppData\Local\Programs\T1M\Glowdrift\glowdrift.exe"
        ));
        assert!(!per_machine(r"C:\Program Files Extra\T1M\glowdrift.exe"));
        assert!(!per_machine(r"C:\Program Filesystem\glowdrift.exe"));
        assert!(!per_machine(r"D:\Program Files\T1M\glowdrift.exe"));
        assert!(!per_machine(
            r"C:\Users\tim\Program Files\T1M\glowdrift.exe"
        ));
    }

    #[test]
    fn empty_or_missing_roots_never_match() {
        let exe = Path::new(r"C:\Program Files\T1M\glowdrift.exe");
        assert!(!is_under_any(exe, &[]));
        assert!(!is_under_any(exe, &[PathBuf::new(), PathBuf::from(r"\")]));
        assert!(!is_under_any(Path::new(""), &roots()));
    }

    #[test]
    fn installer_arguments_without_app_arguments() {
        assert_eq!(installer_arguments(&[]), OsString::from("/P /UPDATE /R"));
    }

    #[test]
    fn installer_arguments_append_app_arguments_last() {
        let args = [OsString::from("--minimized")];
        assert_eq!(
            installer_arguments(&args),
            OsString::from("/P /UPDATE /R /ARGS --minimized")
        );
    }

    #[test]
    fn installer_arguments_quote_what_the_installer_would_misread() {
        assert_eq!(quote_argument("plain"), "plain");
        assert_eq!(quote_argument(""), "\"\"");
        assert_eq!(quote_argument("a b"), "\"a b\"");
        assert_eq!(quote_argument("/x"), "\"/x\"");
        assert_eq!(quote_argument(r"dir with\"), r#""dir with\\""#);
        assert_eq!(quote_argument(r#"say "hi""#), r#""say \"hi\"""#);
    }

    #[test]
    fn only_executables_are_launched() {
        assert!(is_executable_image(b"MZ\x90\x00"));
        assert!(!is_executable_image(b"PK\x03\x04"));
        assert!(!is_executable_image(b""));
    }
}
