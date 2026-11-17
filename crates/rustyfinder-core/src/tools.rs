//! Custom tools: user-defined shell commands run on the selection.
//!
//! A command may contain placeholders (`$SOURCE_PATH`,
//! `$SOURCE_SELECTION_PATHS`…). Each value is put in shell-quoted, as one
//! argument per path or name, so nothing in a file name can split into
//! several arguments or run as code. The command then runs through the
//! user's login shell, exactly as it would in Terminal.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::paths;

/// One tool as defined in Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tool {
    /// Stable identity, so a rename keeps shortcuts and toolbar items.
    pub id: u64,
    pub name: String,
    pub command: String,
    /// Show what the command prints in a window. Off: only failures show.
    pub show_output: bool,
    /// Run once per selected item instead of once with all of them.
    pub per_file: bool,
    /// Key equivalent in the app's shortcut notation; empty for none.
    pub shortcut: String,
    pub in_toolbar: bool,
    pub in_context_menu: bool,
}

impl Default for Tool {
    fn default() -> Self {
        Self {
            id: 0,
            name: "Untitled".into(),
            command: String::new(),
            show_output: true,
            per_file: false,
            shortcut: String::new(),
            in_toolbar: false,
            in_context_menu: true,
        }
    }
}

impl Tool {
    /// A tool needs a name and a command to be saved.
    pub fn is_valid(&self) -> bool {
        !self.name.trim().is_empty() && !self.command.trim().is_empty()
    }
}

/// What the placeholders stand for at the moment a tool runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    /// The active pane's folder.
    pub source_path: String,
    /// The other pane's folder.
    pub target_path: String,
    /// Full paths of the items selected in the active pane.
    pub source_selection: Vec<String>,
    pub target_selection: Vec<String>,
    /// Host and user of the active pane's server; empty on this Mac.
    pub hostname: String,
    pub username: String,
}

/// The placeholders, for the palette in the tool editor.
pub const PLACEHOLDERS: [&str; 8] = [
    "$SOURCE_PATH",
    "$TARGET_PATH",
    "$SOURCE_SELECTION_PATHS",
    "$SOURCE_SELECTION_NAMES",
    "$TARGET_SELECTION_PATHS",
    "$TARGET_SELECTION_NAMES",
    "$SOURCE_HOSTNAME",
    "$SOURCE_USERNAME",
];

fn quoted_list(items: impl Iterator<Item = String>) -> String {
    items.map(|item| paths::shell_escape(&item)).collect::<Vec<_>>().join(" ")
}

fn names(paths_: &[String]) -> impl Iterator<Item = String> + '_ {
    paths_.iter().map(|p| paths::file_name(p).to_string())
}

/// Replace every placeholder in `command`. Only whole names count:
/// `$SOURCE_PATHS` is not a placeholder and is left for the shell.
pub fn expand(command: &str, ctx: &Context) -> String {
    let mut out = String::with_capacity(command.len() + 64);
    let mut rest = command;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let len = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(after.len());
        let value = match &after[..len] {
            "SOURCE_PATH" => Some(paths::shell_escape(&ctx.source_path)),
            "TARGET_PATH" => Some(paths::shell_escape(&ctx.target_path)),
            "SOURCE_SELECTION_PATHS" => Some(quoted_list(ctx.source_selection.iter().cloned())),
            "SOURCE_SELECTION_NAMES" => Some(quoted_list(names(&ctx.source_selection))),
            "TARGET_SELECTION_PATHS" => Some(quoted_list(ctx.target_selection.iter().cloned())),
            "TARGET_SELECTION_NAMES" => Some(quoted_list(names(&ctx.target_selection))),
            "SOURCE_HOSTNAME" => Some(paths::shell_escape(&ctx.hostname)),
            "SOURCE_USERNAME" => Some(paths::shell_escape(&ctx.username)),
            _ => None,
        };
        match value {
            Some(value) => out.push_str(&value),
            None => {
                out.push('$');
                out.push_str(&after[..len]);
            }
        }
        rest = &after[len..];
    }
    out.push_str(rest);
    out
}

/// The command lines one use of the tool runs: one for the whole
/// selection, or one per selected item (none if nothing is selected).
pub fn commands(tool: &Tool, ctx: &Context) -> Vec<String> {
    if !tool.per_file {
        return vec![expand(&tool.command, ctx)];
    }
    ctx.source_selection
        .iter()
        .map(|item| {
            let one = Context { source_selection: vec![item.clone()], ..ctx.clone() };
            expand(&tool.command, &one)
        })
        .collect()
}

/// The saved tool other than `except` whose shortcut is `binding` (in
/// any spelling: `Alt+Cmd+T` and `cmd+alt+t` are the same keys).
pub fn shortcut_holder(tools: &[Tool], binding: &str, except: u64) -> Option<u64> {
    let wanted = crate::shortcuts::normalize(binding).filter(|b| !b.is_empty())?;
    tools
        .iter()
        .find(|t| t.id != except && crate::shortcuts::normalize(&t.shortcut).as_deref() == Some(wanted.as_str()))
        .map(|t| t.id)
}

/// Where an SFTP connection goes, for opening a shell there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SshTarget {
    pub host: String,
    pub user: String,
    pub port: Option<u16>,
}

/// The command line that opens an interactive shell on the server in
/// `remote_path`: `ssh -t … 'cd <path>; exec $SHELL -l'`. The remote
/// command is quoted for the server's shell, then the whole line for the
/// local one, so nothing in a path, user or host name runs as code.
/// `None` for a host that would read as an option.
pub fn ssh_shell_command(target: &SshTarget, remote_path: &str) -> Option<String> {
    let host = target.host.trim();
    if host.is_empty() || host.starts_with('-') || host.contains(char::is_whitespace) {
        return None;
    }
    // Plain single quotes: the server's shell may be any POSIX shell.
    let remote = format!("cd {} || exit 1; exec \"$SHELL\" -l", single_quoted(remote_path));
    let mut line = String::from("ssh -t");
    if let Some(port) = target.port {
        line.push_str(&format!(" -p {port}"));
    }
    if !target.user.is_empty() {
        line.push_str(&format!(" -l {}", single_quoted(&target.user)));
    }
    // "--" ends the options: the host and command can never be taken
    // for one.
    line.push_str(&format!(" -- {} {}", single_quoted(host), single_quoted(&remote)));
    Some(line)
}

/// `text` as one word for any POSIX shell.
fn single_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// How one command ended.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunResult {
    /// Standard output and standard error, in the order they were written.
    pub output: String,
    /// `None` if a signal ended it.
    pub exit_code: Option<i32>,
    /// Ended because the user pressed Stop.
    pub stopped: bool,
}

/// Output beyond this is dropped (the tail is kept), so a runaway command
/// cannot fill memory.
const MAX_OUTPUT: usize = 4 << 20;
/// After Stop: this long to exit on SIGTERM before SIGKILL.
const GRACE: Duration = Duration::from_secs(5);

/// Run `command` through the user's login shell in `cwd`, as Terminal
/// would: the shell's own `$PATH`, globbing and pipes. Blocks until the
/// command ends. `keep_going` returning false stops it: SIGTERM to the
/// whole process group, then SIGKILL if it has not gone after 5 seconds.
pub fn run(command: &str, cwd: &str, keep_going: &dyn Fn() -> bool) -> std::io::Result<RunResult> {
    run_with(command, cwd, inherits_path(), keep_going)
}

/// "Inherit $PATH from the login shell" (Settings → Tools), on by default.
static INHERIT_PATH: AtomicBool = AtomicBool::new(true);

pub fn set_inherit_path(on: bool) {
    INHERIT_PATH.store(on, Ordering::Relaxed);
}

pub fn inherits_path() -> bool {
    INHERIT_PATH.load(Ordering::Relaxed)
}

/// The search path apps started from the Dock get.
pub const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Like `run`. With `inherit_path` the command goes through the login
/// shell and gets its `$PATH`; without, a plain `/bin/sh` with the system
/// search path runs it.
pub fn run_with(
    command: &str,
    cwd: &str,
    inherit_path: bool,
    keep_going: &dyn Fn() -> bool,
) -> std::io::Result<RunResult> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    let (mut reader, writer) = std::io::pipe()?;
    let mut child = {
        let mut cmd = if inherit_path {
            let mut cmd = Command::new(shell);
            cmd.args(["-l", "-c", command]);
            cmd
        } else {
            let mut cmd = Command::new("/bin/sh");
            cmd.args(["-c", command]).env("PATH", SYSTEM_PATH);
            cmd
        };
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(writer.try_clone()?)
            .stderr(writer)
            // Its own group, so Stop reaches everything it started.
            .process_group(0);
        cmd.spawn()?
        // `cmd` is dropped here, closing this side's copies of the pipe.
    };
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let pump = std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 || tx.send(buffer[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let group = child.id() as libc::pid_t;
    let mut bytes: Vec<u8> = Vec::new();
    let mut stop_sent: Option<Instant> = None;
    let mut killed = false;
    let mut exited: Option<Instant> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                bytes.extend_from_slice(&chunk);
                if bytes.len() > MAX_OUTPUT {
                    let cut = bytes.len() - MAX_OUTPUT;
                    bytes.drain(..cut);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // The shell is gone but something it left behind still
                // holds the pipe open: do not wait for that.
                if child.try_wait()?.is_some() {
                    match exited {
                        Some(at) if at.elapsed() > Duration::from_millis(300) => break,
                        None => exited = Some(Instant::now()),
                        _ => {}
                    }
                }
            }
        }
        match stop_sent {
            None if !keep_going() => {
                // SAFETY: plain signal to a process group this function created.
                unsafe { libc::killpg(group, libc::SIGTERM) };
                stop_sent = Some(Instant::now());
            }
            Some(at) if !killed && at.elapsed() >= GRACE => {
                // SAFETY: as above.
                unsafe { libc::killpg(group, libc::SIGKILL) };
                killed = true;
            }
            _ => {}
        }
    }
    let status = child.wait()?;
    drop(rx);
    let _ = pump.join();
    Ok(RunResult {
        output: String::from_utf8_lossy(&bytes).into_owned(),
        exit_code: status.code(),
        stopped: stop_sent.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &str, names: &[&str]) -> Context {
        Context {
            source_path: dir.to_string(),
            target_path: "/tmp/other side".into(),
            source_selection: names.iter().map(|n| format!("{dir}/{n}")).collect(),
            target_selection: vec!["/tmp/other side/t.txt".into()],
            hostname: String::new(),
            username: String::new(),
        }
    }

    fn sh(command: &str, cwd: &str) -> RunResult {
        run(command, cwd, &|| true).unwrap()
    }

    #[test]
    fn tl01_names_stay_whole_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let c = ctx(cwd, &["a b.txt", "c\"d.txt"]);
        let result = sh(&expand("echo $SOURCE_SELECTION_NAMES", &c), cwd);
        assert_eq!(result.output, "a b.txt c\"d.txt\n");
        assert_eq!(result.exit_code, Some(0));
        // Exactly two arguments reached the command.
        let counted = sh(&expand("set -- $SOURCE_SELECTION_NAMES; echo $#", &c), cwd);
        assert_eq!(counted.output.trim(), "2");
    }

    #[test]
    fn hostile_names_cannot_inject_or_split() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let nasty = ["$(touch pwned)", "`touch pwned2`", "; touch pwned3 #", "new\nline", "it's", "日本 語", "-n"];
        let c = ctx(cwd, &nasty);
        let result = sh(&expand("for a in $SOURCE_SELECTION_NAMES; do printf '%s|' \"$a\"; done", &c), cwd);
        assert_eq!(result.output, nasty.iter().map(|n| format!("{n}|")).collect::<String>());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "nothing in a name was run");
    }

    #[test]
    fn placeholders_are_whole_words_and_paths_are_quoted() {
        let c = ctx("/tmp/my dir", &["x y"]);
        assert_eq!(expand("cd $SOURCE_PATH && ls", &c), "cd /tmp/my\\ dir && ls");
        assert_eq!(expand("cp $SOURCE_SELECTION_PATHS $TARGET_PATH", &c), "cp /tmp/my\\ dir/x\\ y /tmp/other\\ side");
        assert_eq!(expand("$TARGET_SELECTION_NAMES", &c), "t.txt");
        // Not placeholders: left for the shell.
        assert_eq!(
            expand("echo $HOME $SOURCE_PATHS ${SOURCE_PATH} $", &c),
            "echo $HOME $SOURCE_PATHS ${SOURCE_PATH} $"
        );
        // On this Mac the server placeholders are empty arguments.
        assert_eq!(expand("ssh $SOURCE_USERNAME@$SOURCE_HOSTNAME", &c), "ssh ''@''");
        // Nothing selected: the list is simply empty.
        assert_eq!(expand("echo [$SOURCE_SELECTION_PATHS]", &ctx("/d", &[])), "echo []");
    }

    #[test]
    fn tl02_per_file_runs_once_per_item() {
        let tool =
            Tool { command: "echo $SOURCE_SELECTION_NAMES in $SOURCE_PATH".into(), per_file: true, ..Tool::default() };
        let c = ctx("/d", &["1", "2", "3"]);
        assert_eq!(commands(&tool, &c), ["echo 1 in /d", "echo 2 in /d", "echo 3 in /d"]);
        assert!(commands(&tool, &ctx("/d", &[])).is_empty());
        let once = Tool { per_file: false, ..tool };
        assert_eq!(commands(&once, &c), ["echo 1 2 3 in /d"]);
    }

    #[test]
    fn tl04_the_exit_code_and_both_streams_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let result = sh("echo out; echo err >&2; exit 2", dir.path().to_str().unwrap());
        assert_eq!(result.output, "out\nerr\n");
        assert_eq!(result.exit_code, Some(2));
        assert!(!result.stopped);
    }

    #[test]
    fn it_runs_in_the_given_folder_through_a_login_shell() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        std::fs::write(cwd.join("one.txt"), "").unwrap();
        let result = sh("pwd -P; echo *.txt; echo a | tr a b", cwd.to_str().unwrap());
        // The folder, a glob and a pipe, all as in Terminal.
        assert_eq!(result.output, format!("{}\none.txt\nb\n", cwd.to_str().unwrap()));
    }

    #[test]
    fn stop_ends_a_running_command() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let result = run("echo begun; sleep 30; echo never", dir.path().to_str().unwrap(), &|| {
            started.elapsed() < Duration::from_millis(400)
        })
        .unwrap();
        assert!(result.stopped);
        assert!(started.elapsed() < Duration::from_secs(4), "SIGTERM was enough");
        assert_eq!(result.output, "begun\n");
        assert_eq!(result.exit_code, None);
    }

    #[test]
    fn tl03_without_the_login_path_the_system_path_is_used() {
        // settings-defaults §4: the switch decides where $PATH comes from.
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let plain = run_with("echo $PATH", cwd, false, &|| true).unwrap();
        assert_eq!(plain.output.trim(), SYSTEM_PATH);
        // The login shell sets up its own path (it always includes the system one).
        let login = run_with("echo $PATH", cwd, true, &|| true).unwrap();
        assert!(login.output.contains("/usr/bin"), "{}", login.output);
        assert!(inherits_path(), "on by default");
    }

    #[test]
    fn a_shortcut_held_by_another_tool_is_found_in_any_spelling() {
        let tools = [
            Tool { id: 1, shortcut: "ctrl+alt+t".into(), ..Tool::default() },
            Tool { id: 2, shortcut: String::new(), ..Tool::default() },
        ];
        assert_eq!(shortcut_holder(&tools, "Alt+Ctrl+T", 2), Some(1));
        assert_eq!(shortcut_holder(&tools, "ctrl+alt+t", 1), None, "a tool does not clash with itself");
        assert_eq!(shortcut_holder(&tools, "", 2), None, "no shortcut clashes with nothing");
        assert_eq!(shortcut_holder(&tools, "cmd+k", 2), None);
    }

    #[test]
    fn the_ssh_line_quotes_everything_and_ends_the_options() {
        let target = SshTarget { host: "example.org".into(), user: "bob".into(), port: Some(2222) };
        assert_eq!(
            ssh_shell_command(&target, "/srv/my files").unwrap(),
            r#"ssh -t -p 2222 -l 'bob' -- 'example.org' 'cd '\''/srv/my files'\'' || exit 1; exec "$SHELL" -l'"#
        );
        // Split the line as the local shell would, then run the remote part
        // as the server's shell would: the path arrives whole and nothing
        // in it runs.
        let dir = tempfile::tempdir().unwrap();
        let nasty = dir.path().join("a'b $(touch pwned) `touch p2`;x");
        std::fs::create_dir(&nasty).unwrap();
        let line = ssh_shell_command(&SshTarget { host: "h".into(), ..SshTarget::default() }, nasty.to_str().unwrap())
            .unwrap();
        let words = run_with(&format!("set -- {line}; printf '%s\\n' \"$@\""), "/", false, &|| true).unwrap();
        let args: Vec<&str> = words.output.lines().collect();
        assert_eq!(args[..4], ["ssh", "-t", "--", "h"]);
        let remote = args[4].replace(r#"exec "$SHELL" -l"#, "pwd -P");
        let ran = run_with(&remote, "/", false, &|| true).unwrap();
        assert_eq!(ran.output.trim(), nasty.canonicalize().unwrap().to_str().unwrap());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "nothing in the path was run");
        // A host that would read as an option is refused.
        let option = SshTarget { host: "-oProxyCommand=x".into(), ..SshTarget::default() };
        assert!(ssh_shell_command(&option, "/").is_none());
        assert!(ssh_shell_command(&SshTarget::default(), "/").is_none());
    }

    #[test]
    fn a_tool_needs_a_name_and_a_command() {
        assert!(!Tool::default().is_valid());
        let tool = Tool { command: "ls".into(), ..Tool::default() };
        assert!(tool.is_valid());
        assert_eq!(Tool::default().name, "Untitled");
        // Older saved tools without the newer fields still load.
        let loaded: Tool = serde_json::from_str(r#"{"id":3,"name":"x","command":"y"}"#).unwrap();
        assert!(loaded.show_output && loaded.in_context_menu && !loaded.per_file);
    }
}
