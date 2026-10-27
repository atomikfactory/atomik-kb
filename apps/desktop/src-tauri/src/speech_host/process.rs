//! Where the `kb-speech` sidecar comes from and how it is started: the seam the tests replace.
//!
//! [`ProcessLauncher`] is the real one. It finds the executable next to the app's own and the voice activity
//! model in the places the installer may put it (`SpeechShell::sidecar`, `SpeechShell::vad`), checks the model's
//! SHA-256 (644 KB, hashed at every session), and starts the process with its stdin, stdout and stderr piped, and
//! on Windows with `CREATE_NO_WINDOW` so no console window flashes. The tests use a launcher whose "process" is
//! a thread behind in-process pipes (`speech_host::tests`), so the session logic runs without the DLLs.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use kb_speech::catalog::VAD_FILE;
use sha2::{Digest, Sha256};

use super::SpeechShell;

/// The `CREATE_NO_WINDOW` process creation flag of Windows.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A running sidecar, as far as the session needs to control it.
pub trait SidecarProcess: Send {
    /// Ends the process now (a no-op when it has ended).
    fn kill(&mut self);

    /// Whether the process has ended, without waiting for it.
    fn has_exited(&mut self) -> bool;
}

/// The pipes and the handle of a started sidecar.
pub struct Sidecar {
    /// Frames to the sidecar.
    pub stdin: Box<dyn Write + Send>,
    /// Frames from the sidecar.
    pub stdout: Box<dyn Read + Send>,
    /// The sidecar's diagnostics (never shown to the webview, never logged as they are).
    pub stderr: Box<dyn Read + Send>,
    /// The process.
    pub process: Box<dyn SidecarProcess>,
}

/// The sidecar executable and the voice activity model that were found and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    /// The executable.
    pub exe: PathBuf,
    /// `silero_vad.onnx`.
    pub vad: PathBuf,
}

/// Finds and starts the sidecar.
pub trait Launcher: Send + Sync {
    /// The sidecar and the voice activity model, when both are there and the model is the pinned one; `None`
    /// otherwise (`engineMissing`). Blocking (it reads the model's 644 KB).
    fn locate(&self, shell: &SpeechShell) -> Option<Located>;

    /// Starts the sidecar at `exe`.
    ///
    /// # Errors
    ///
    /// The operating system's error when the process cannot be started.
    fn launch(&self, exe: &Path) -> io::Result<Sidecar>;
}

/// The real launcher: the executable next to the app's own.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessLauncher;

impl Launcher for ProcessLauncher {
    fn locate(&self, shell: &SpeechShell) -> Option<Located> {
        let exe = shell.sidecar()?;
        let vad = shell.vad()?;
        vad_is_pinned(&vad).then_some(Located { exe, vad })
    }

    fn launch(&self, exe: &Path) -> io::Result<Sidecar> {
        let mut command = Command::new(exe);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn()?;
        match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
            (Some(stdin), Some(stdout), Some(stderr)) => Ok(Sidecar {
                stdin: Box::new(stdin),
                stdout: Box::new(stdout),
                stderr: Box::new(stderr),
                process: Box::new(ChildProcess(child)),
            }),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                Err(io::Error::other("the sidecar's pipes are not there"))
            }
        }
    }
}

/// A [`Child`] as a [`SidecarProcess`].
struct ChildProcess(Child);

impl SidecarProcess for ChildProcess {
    fn kill(&mut self) {
        // Killing a process that has ended is an error that means nothing here; waiting afterwards releases the
        // handle.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }

    fn has_exited(&mut self) -> bool {
        // A failure to ask is treated as "gone": the caller only waits for the process to leave.
        !matches!(self.0.try_wait(), Ok(None))
    }
}

/// The SHA-256 of a file, as lower-case hex.
///
/// # Errors
///
/// The error of reading the file.
pub fn sha256_of_file(path: &Path) -> io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut block = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut block)?;
        if read == 0 {
            break;
        }
        hasher.update(&block[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// `bytes` as lower-case hex.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// Whether the file at `path` is the pinned voice activity model (size, then SHA-256).
fn vad_is_pinned(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.len() == VAD_FILE.bytes)
        && sha256_of_file(path).is_ok_and(|hash| hash == VAD_FILE.sha256)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_hashed_the_way_the_catalog_writes_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abc");
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_of_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // More than one block, and nothing at all.
        std::fs::write(&path, vec![0_u8; 200_000]).unwrap();
        assert_eq!(sha256_of_file(&path).unwrap().len(), 64);
        std::fs::write(&path, b"").unwrap();
        assert_eq!(
            sha256_of_file(&path).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert!(sha256_of_file(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn hex_is_two_lower_case_digits_a_byte() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(hex(&[]), "");
    }

    #[test]
    fn a_voice_activity_file_that_is_not_the_pinned_one_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(VAD_FILE.name);
        // The right size but other content: the size alone is not enough.
        std::fs::File::create(&path)
            .unwrap()
            .set_len(VAD_FILE.bytes)
            .unwrap();
        assert!(!vad_is_pinned(&path));
        std::fs::write(&path, b"short").unwrap();
        assert!(!vad_is_pinned(&path));
        assert!(!vad_is_pinned(&dir.path().join("missing.onnx")));
    }

    /// The real voice activity model, when `scripts/fetch-speech-deps.ps1` fetched it (`KB_SPEECH_VAD`).
    #[test]
    fn the_fetched_voice_activity_model_is_the_pinned_one() {
        let Some(path) = std::env::var_os(super::super::VAD_ENV).map(PathBuf::from) else {
            return;
        };
        if path.is_absolute() && path.is_file() {
            assert!(vad_is_pinned(&path));
        }
    }
}
