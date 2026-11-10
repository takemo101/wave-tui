//! Bounded, nonblocking CLI transport; no shell is used by the adapter.
use std::io::{ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

struct Reap(Child);

impl Drop for Reap {
    fn drop(&mut self) {
        // Includes timeout, cancellation, invalid output and early I/O errors.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(super) fn run(
    command: &mut Command,
    budget: Duration,
    limit: usize,
    stop: &AtomicBool,
) -> Option<Vec<u8>> {
    let deadline = Instant::now() + budget;
    if stop.load(Ordering::Acquire) {
        return None;
    }
    let mut child = Reap(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?,
    );
    let mut stdout = child.0.stdout.take()?;
    let fd = stdout.as_raw_fd();
    // ChildStdout owns this live fd for the entire operation. Nonblocking
    // reads let a single thread enforce both a total deadline and byte cap.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return None;
    }
    let mut bytes = Vec::new();
    let mut eof = false;
    loop {
        if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            return None;
        }
        let mut buffer = [0_u8; 8192];
        loop {
            if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
                return None;
            }
            match stdout.read(&mut buffer) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => {
                    if n > limit.saturating_sub(bytes.len()) {
                        return None;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return None,
            }
        }
        if let Some(status) = child.0.try_wait().ok()? {
            if !status.success() {
                return None;
            }
            if eof {
                return Some(bytes);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn returns_only_complete_successful_stdout() {
        let stop = AtomicBool::new(false);
        assert_eq!(
            run(
                &mut shell("printf '{\"entries\":[]}'"),
                Duration::from_secs(1),
                1024,
                &stop
            ),
            Some(br#"{"entries":[]}"#.to_vec())
        );
        assert_eq!(
            run(
                &mut shell("printf private >&2; printf partial; exit 1"),
                Duration::from_secs(1),
                1024,
                &stop
            ),
            None
        );
        assert_eq!(
            run(
                &mut Command::new("/nonexistent/rex"),
                Duration::from_secs(1),
                1024,
                &stop
            ),
            None
        );
    }

    #[test]
    fn oversized_or_trickling_output_and_hung_commands_are_bounded() {
        let stop = AtomicBool::new(false);
        for script in [
            "printf 123456789",
            "while :; do printf x; done",
            "exec sleep 10",
        ] {
            let start = Instant::now();
            assert_eq!(
                run(&mut shell(script), Duration::from_millis(100), 8, &stop),
                None
            );
            assert!(start.elapsed() < Duration::from_secs(2), "{script}");
        }
    }

    #[test]
    fn cancellation_kills_and_reaps_an_already_running_child() {
        use std::sync::Arc;
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let pid_file =
            std::env::temp_dir().join(format!("wave-tui-rex-{}-{unique}.pid", std::process::id()));
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "echo $$ > \"$1\"; exec sleep 10", "rex-test"])
            .arg(&pid_file);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = std::thread::spawn(move || {
            run(&mut command, Duration::from_secs(3), 1024, &worker_stop)
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        let pid = loop {
            if let Ok(contents) = std::fs::read_to_string(&pid_file) {
                if let Ok(pid) = contents.trim().parse::<libc::pid_t>() {
                    break pid;
                }
            }
            assert!(Instant::now() < deadline, "test child never started");
            std::thread::sleep(Duration::from_millis(5));
        };
        let start = Instant::now();
        stop.store(true, Ordering::Release);
        assert!(worker.join().unwrap().is_none());
        assert!(start.elapsed() < Duration::from_secs(1));
        // kill(0) sends no signal; ESRCH proves the child is gone, not a zombie.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        std::fs::remove_file(pid_file).unwrap();
    }

    #[test]
    fn cancellation_does_not_start_a_command() {
        assert_eq!(
            run(
                &mut shell("printf ran"),
                Duration::from_secs(1),
                1024,
                &AtomicBool::new(true)
            ),
            None
        );
    }
}
