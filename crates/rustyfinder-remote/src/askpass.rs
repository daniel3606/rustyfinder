//! Relaying ssh's questions to the app.
//!
//! ssh is started with `SSH_ASKPASS` set to a helper program and
//! `SSH_ASKPASS_REQUIRE=force`. Whenever ssh needs something from the user
//! (a password, a key passphrase, a verification code, a yes/no on an
//! unknown host key) it runs that helper with the question as its only
//! argument and reads the answer from the helper's standard output.
//!
//! The helper does no thinking. It connects to a Unix socket owned by the
//! running app (path in `RUSTYFINDER_ASKPASS_SOCKET`), sends the question and
//! prints whatever comes back. The app end is `AskpassServer`.

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Environment variable carrying the socket path to the helper.
pub const SOCKET_ENV: &str = "RUSTYFINDER_ASKPASS_SOCKET";

/// What ssh is asking for, worked out from the wording of its question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Password,
    /// Passphrase of a private key.
    Passphrase,
    /// "Are you sure you want to continue connecting (yes/no)?" for a host
    /// whose key is not known yet. The text carries the fingerprint.
    HostKey,
    /// Anything else, e.g. a "Verification code:" second factor.
    Other,
}

/// Answers ssh's questions. Called on a background thread; may block for
/// as long as the user takes.
pub trait Prompter: Send + Sync {
    /// Return the answer, or `None` to cancel the connection attempt. For
    /// `HostKey` the answer is "yes" or "no".
    fn prompt(&self, text: &str, kind: PromptKind) -> Option<String>;
}

/// Classify a question by its wording.
pub fn classify(text: &str) -> PromptKind {
    let lower = text.to_lowercase();
    if lower.contains("(yes/no") {
        PromptKind::HostKey
    } else if lower.contains("passphrase") {
        PromptKind::Passphrase
    } else if lower.contains("password") {
        PromptKind::Password
    } else {
        PromptKind::Other
    }
}

/// The app's end of the relay: a private Unix socket and a thread that
/// answers the helper.
pub struct AskpassServer {
    dir: PathBuf,
    socket: PathBuf,
    stop: Arc<AtomicBool>,
}

impl AskpassServer {
    /// `password` answers the first password question without asking the
    /// user. A second password question means it was wrong, and goes to
    /// the user. Everything else always goes to `prompter`.
    pub fn start(password: Option<String>, prompter: Arc<dyn Prompter>) -> std::io::Result<Self> {
        // A fresh directory only this user can enter: nobody else can
        // connect to the socket and read a password out of it.
        let dir = std::env::temp_dir().join(format!(
            "rustyfinder-askpass-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        let socket = dir.join("s");
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;

        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let password = Mutex::new(password);
        thread::Builder::new().name("rustyfinder-askpass".into()).spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = answer(stream, &password, &*prompter);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            }
        })?;
        Ok(Self { dir, socket, stop })
    }

    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket
    }
}

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Drop for AskpassServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_dir(&self.dir);
    }
}

/// Serve one question: read it, decide, write `A<answer>` or `C` (cancel).
fn answer(mut stream: UnixStream, password: &Mutex<Option<String>>, prompter: &dyn Prompter) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut text = String::new();
    stream.read_to_string(&mut text)?;
    let kind = classify(&text);
    let reply = match kind {
        // The password typed in the Connect panel is used once.
        PromptKind::Password => password.lock().unwrap().take().or_else(|| prompter.prompt(&text, kind)),
        _ => prompter.prompt(&text, kind),
    };
    match reply {
        Some(value) => {
            stream.write_all(b"A")?;
            stream.write_all(value.as_bytes())?;
        }
        None => stream.write_all(b"C")?,
    }
    stream.flush()
}

/// The helper's whole job, to be called from a `main`. Sends `prompt` to
/// the app and prints the answer. Returns the process exit code: 0 with an
/// answer on stdout, 1 if the user cancelled or the app could not be reached.
pub fn relay_main(prompt: &str) -> i32 {
    let Ok(socket) = std::env::var(SOCKET_ENV) else {
        return 1;
    };
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return 1;
    };
    if stream.write_all(prompt.as_bytes()).is_err() || stream.shutdown(Shutdown::Write).is_err() {
        return 1;
    }
    let mut reply = Vec::new();
    if stream.read_to_end(&mut reply).is_err() {
        return 1;
    }
    match reply.split_first() {
        Some((b'A', answer)) => {
            let mut out = std::io::stdout();
            if out.write_all(answer).is_err() || out.write_all(b"\n").is_err() || out.flush().is_err() {
                return 1;
            }
            0
        }
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Mutex<Vec<(String, PromptKind)>>, Option<String>);

    impl Prompter for Fixed {
        fn prompt(&self, text: &str, kind: PromptKind) -> Option<String> {
            self.0.lock().unwrap().push((text.to_string(), kind));
            self.1.clone()
        }
    }

    /// Ask the server as the helper would, returning the raw reply.
    fn ask(server: &AskpassServer, question: &str) -> Vec<u8> {
        let mut stream = UnixStream::connect(server.socket_path()).unwrap();
        stream.write_all(question.as_bytes()).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).unwrap();
        reply
    }

    #[test]
    fn questions_are_classified_by_wording() {
        assert_eq!(classify("bob@example.org's password: "), PromptKind::Password);
        assert_eq!(classify("Enter passphrase for key '/Users/b/.ssh/id_ed25519': "), PromptKind::Passphrase);
        assert_eq!(
            classify("The authenticity of host 'h' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? "),
            PromptKind::HostKey
        );
        assert_eq!(classify("Verification code: "), PromptKind::Other);
    }

    #[test]
    fn the_given_password_is_used_once_then_the_user_is_asked() {
        let prompter = Arc::new(Fixed(Mutex::new(Vec::new()), Some("typed".into())));
        let server = AskpassServer::start(Some("s3cret".into()), prompter.clone()).unwrap();
        assert_eq!(ask(&server, "bob@h's password: "), b"As3cret");
        assert!(prompter.0.lock().unwrap().is_empty());
        // Asked again: the first answer was wrong, so the user decides.
        assert_eq!(ask(&server, "bob@h's password: "), b"Atyped");
        assert_eq!(prompter.0.lock().unwrap().len(), 1);
        // Other questions never get the password.
        assert_eq!(ask(&server, "Verification code: "), b"Atyped");
        assert_eq!(prompter.0.lock().unwrap()[1].1, PromptKind::Other);
    }

    #[test]
    fn cancelling_is_reported_and_the_socket_is_private_and_cleaned_up() {
        let prompter = Arc::new(Fixed(Mutex::new(Vec::new()), None));
        let server = AskpassServer::start(None, prompter).unwrap();
        assert_eq!(ask(&server, "Enter passphrase for key 'k': "), b"C");
        let dir = server.socket_path().parent().unwrap().to_path_buf();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        drop(server);
        assert!(!dir.exists());
    }
}
