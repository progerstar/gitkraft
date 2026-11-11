//! Running external commands with a time limit.

use std::io::Read;
use std::process::{Child, Command, Output, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often a running command is checked on.
const POLL: Duration = Duration::from_millis(100);

/// Time a command gets to exit after SIGTERM before it is killed.
#[cfg(unix)]
const GRACE: Duration = Duration::from_secs(5);

/// Like [`Command::output`], but gives up once `timeout` has passed and
/// returns `Ok(None)`. The command is first asked to stop (SIGTERM on Unix,
/// so git removes its lock files and ends its ssh), then killed.
pub(crate) fn output_with_timeout(
    cmd: &mut Command,
    timeout: Duration,
) -> std::io::Result<Option<Output>> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    // Read both pipes on their own threads so a chatty command cannot
    // block on a full pipe.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            stop(&mut child);
            // The readers end once the pipes close; nobody waits for them.
            return Ok(None);
        }
        std::thread::sleep(POLL);
    };
    Ok(Some(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    }))
}

/// Ask `child` to exit, and kill it if it is still running after [`GRACE`].
fn stop(child: &mut Child) {
    #[cfg(unix)]
    {
        // std only sends SIGKILL, which leaves git no chance to clean up.
        // Not reaped yet, so the pid cannot belong to another process.
        let _ = Command::new("kill")
            .arg(child.id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let deadline = Instant::now() + GRACE;
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(POLL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn returns_the_output_of_a_command_that_finishes() {
        let out = output_with_timeout(
            Command::new("sh").args(["-c", "echo out; echo err >&2; exit 3"]),
            Duration::from_secs(10),
        )
        .unwrap()
        .unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, b"err\n");
    }

    #[test]
    fn stops_a_command_that_runs_too_long() {
        let started = Instant::now();
        let out = output_with_timeout(Command::new("sleep").arg("30"), Duration::from_millis(300))
            .unwrap();
        assert!(out.is_none());
        // Stopped by SIGTERM, without waiting out the grace period.
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }
}
