//! Picker stdout purity — the selection pipe carries selections, nothing else.
//!
//! Regression for the 2026-06-11 CPR-leak incident: skim 3.x's stack
//! (ratatui `CrosstermBackend::get_cursor_position` → `crossterm::cursor::position`)
//! writes its `ESC[6n` cursor query to the *process stdout*. Every skim-tab
//! binary runs with stdout piped — the pipe IS the selection protocol — so the
//! query bytes landed in the pipe ahead of the selection. Consumers (frost's
//! picker splice) inserted them verbatim into the line editor: the operator
//! saw `^[[6n` prefixed to every recalled command, and a cancelled picker
//! produced a "selection" consisting of the query alone, which then got
//! accepted into shell history. The unanswerable query (the terminal never
//! saw it) also stalled teardown on a 2-second crossterm timeout.
//!
//! `TtyStdoutGuard` (lib.rs) holds fd 1 on the controlling tty for the
//! duration of every skim run. These rows drive the REAL binaries under a
//! pty, answer cursor queries the way a real terminal does, and assert the
//! protocol invariant: **no escape byte ever reaches the selection pipe**.

#![cfg(unix)]

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

const CPR_QUERY: &[u8] = b"\x1b[6n";
const CPR_ANSWER: &[u8] = b"\x1b[24;1R";

/// What the driver should do once the picker has painted its candidates.
#[derive(Clone, Copy)]
enum Interaction {
    /// Press Enter — accept the top candidate.
    Accept,
    /// Press Esc — cancel without selecting.
    Cancel,
}

struct RunOutcome {
    /// Raw bytes the binary wrote to its (piped) stdout.
    pipe: Vec<u8>,
    /// Wall-clock from the interaction keypress to child EOF — the teardown
    /// window where the unanswerable-query stall used to live.
    teardown: Duration,
}

/// Drive one skim-tab binary under a pty: stdout → pipe (exactly frost's
/// picker protocol), tty answered like a real terminal, one interaction.
fn drive_picker(
    exe_path: &str,
    histfile: &std::path::Path,
    wait_for: &str,
    interaction: Interaction,
) -> Option<RunOutcome> {
    use nix::pty::ForkptyResult;
    use std::ffi::CString;

    let home = tempfile::tempdir().expect("tempdir");
    let exe = CString::new(exe_path).unwrap();
    let argv = [exe.clone()];
    let envp: Vec<CString> = vec![
        CString::new("TERM=xterm-256color").unwrap(),
        CString::new(format!("HOME={}", home.path().display())).unwrap(),
        CString::new(format!("HISTFILE={}", histfile.display())).unwrap(),
        CString::new("PATH=/usr/bin:/bin").unwrap(),
    ];

    // The selection pipe — the child's stdout, exactly as frost wires it.
    let (pipe_r, pipe_w) = nix::unistd::pipe().expect("pipe");

    // skim exits immediately under a 0×0 pty — size it like a terminal.
    let winsize = nix::pty::Winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    // SAFETY: the child performs only async-signal-safe operations between
    // fork and exec (dup2 / execve / _exit), per fork-in-threaded-process rules.
    let fork = unsafe { nix::pty::forkpty(Some(&winsize), None) };
    let (child, master) = match fork {
        Ok(ForkptyResult::Parent { child, master }) => (child, master),
        Ok(ForkptyResult::Child) => {
            drop(pipe_r);
            let _ = nix::unistd::dup2(pipe_w.as_raw_fd(), 1);
            let _ = nix::unistd::execve(&exe, &argv, &envp);
            unsafe { libc::_exit(127) };
        }
        Err(e) => {
            eprintln!("SKIP picker_stdout_purity: forkpty unavailable: {e}");
            return None;
        }
    };
    drop(pipe_w);

    nix::fcntl::fcntl(
        master.as_raw_fd(),
        nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
    )
    .expect("set O_NONBLOCK on pty master");
    let mut master = std::fs::File::from(master);

    let mut transcript: Vec<u8> = Vec::new();
    let mut answered = 0usize;
    let mut interacted_at: Option<Instant> = None;
    let start = Instant::now();
    let total = Duration::from_secs(15);
    let mut eof_at = None;

    while start.elapsed() < total {
        let mut buf = [0u8; 4096];
        match master.read(&mut buf) {
            Ok(0) => {
                eof_at = Some(Instant::now());
                break;
            }
            Ok(n) => transcript.extend_from_slice(&buf[..n]),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                eof_at = Some(Instant::now());
                break;
            }
        }
        // Answer every cursor-position query like a real terminal would.
        let queries = transcript
            .windows(CPR_QUERY.len())
            .filter(|w| *w == CPR_QUERY)
            .count();
        while answered < queries {
            let _ = master.write_all(CPR_ANSWER);
            answered += 1;
        }
        // Once the candidate list is painted, perform the interaction.
        if interacted_at.is_none()
            && String::from_utf8_lossy(&transcript).contains(wait_for)
        {
            std::thread::sleep(Duration::from_millis(150));
            let key: &[u8] = match interaction {
                Interaction::Accept => b"\r",
                Interaction::Cancel => b"\x1b",
            };
            let _ = master.write_all(key);
            interacted_at = Some(Instant::now());
        }
    }

    let teardown = match (interacted_at, eof_at) {
        (Some(i), Some(e)) => e.duration_since(i),
        _ => {
            let _ = nix::sys::signal::kill(child, nix::sys::signal::Signal::SIGKILL);
            panic!(
                "picker never reached interaction/EOF (interacted: {}, eof: {}); transcript tail: {:?}",
                interacted_at.is_some(),
                eof_at.is_some(),
                String::from_utf8_lossy(
                    &transcript[transcript.len().saturating_sub(200)..]
                ),
            );
        }
    };
    let _ = nix::sys::wait::waitpid(child, None);

    let mut pipe = Vec::new();
    let mut pipe_r = std::fs::File::from(pipe_r);
    let _ = pipe_r.read_to_end(&mut pipe);
    Some(RunOutcome { pipe, teardown })
}

fn write_history(entries: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("history");
    let mut body = String::new();
    for (i, e) in entries.iter().enumerate() {
        body.push_str(&format!(": {}:0;{}\n", 1781151000 + i, e));
    }
    std::fs::write(&path, body).expect("write history");
    (dir, path)
}

/// Accept path: the pipe carries the selected entry and ONLY the selected
/// entry — zero escape bytes — and teardown is instant (no unanswerable
/// query left stalling on a 2s timeout).
#[test]
fn history_accept_pipe_is_pure() {
    let (_dir, hist) = write_history(&["git push origin main", "echo hello"]);
    let Some(out) = drive_picker(
        env!("CARGO_BIN_EXE_skim-history"),
        &hist,
        "echo hello",
        Interaction::Accept,
    ) else {
        return; // forkpty unavailable — environment skip, logged above
    };
    assert!(
        !out.pipe.contains(&0x1b),
        "escape byte leaked into the selection pipe: {:?}",
        String::from_utf8_lossy(&out.pipe)
    );
    let sel = String::from_utf8_lossy(&out.pipe);
    assert!(
        sel == "git push origin main" || sel == "echo hello",
        "selection is not a history entry: {sel:?}"
    );
    assert!(
        out.teardown < Duration::from_millis(1800),
        "teardown stalled {}ms — the unanswerable-query timeout is back",
        out.teardown.as_millis()
    );
}

/// Cancel path: the pipe is COMPLETELY empty. Pre-fix, a cancelled picker
/// returned the bare `ESC[6n` query as a non-empty "selection", which frost
/// spliced into the line and the operator accepted into shell history.
#[test]
fn history_cancel_pipe_is_empty() {
    let (_dir, hist) = write_history(&["git push origin main", "echo hello"]);
    let Some(out) = drive_picker(
        env!("CARGO_BIN_EXE_skim-history"),
        &hist,
        "echo hello",
        Interaction::Cancel,
    ) else {
        return;
    };
    assert!(
        out.pipe.is_empty(),
        "cancelled picker wrote {} bytes to the selection pipe: {:?}",
        out.pipe.len(),
        String::from_utf8_lossy(&out.pipe)
    );
}
