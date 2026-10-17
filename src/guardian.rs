//! Keeps the slime alive. The exe starts itself again as a worker (`--worker`) and, if
//! that crashes, starts a fresh one (`--worker --respawned`). Quitting from the menu, or
//! ending it in Task Manager, stops it for good. Panics are appended to
//! %APPDATA%\SlimeCompanion\crash.log so they can be fixed.

use std::collections::VecDeque;
use std::process::Command;
use std::time::{Duration, Instant};

/// More crashes than this within a minute and it stops trying.
const MAX_RESTARTS: usize = 5;

fn log_path() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|d| std::path::PathBuf::from(d).join("SlimeCompanion").join("crash.log"))
}

fn log(line: &str) {
    use std::io::Write;
    let Some(path) = log_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Keep the log small.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let exe = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        let _ = writeln!(f, "[unix {secs}] {}: {line}", exe.unwrap_or_default());
    }
}

/// Whether an exit code is a crash (an NTSTATUS error, such as an abort after a panic
/// or an access violation) rather than a normal quit or being ended from Task Manager.
pub fn is_crash(code: Option<i32>) -> bool {
    // (-1 is what kill tools such as Stop-Process use: deliberate, not a crash.)
    code.is_some_and(|c| (c as u32) >= 0xC000_0000 && c != -1)
}

/// Runs `app` under the guardian. `app` gets whether this is a restart after a crash.
pub fn run(app: fn(bool)) {
    std::panic::set_hook(Box::new(|info| log(&format!("panic: {info}"))));
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--worker") || std::env::var_os("SLIME_NO_GUARD").is_some() {
        let respawned = args.iter().any(|a| a == "--respawned");
        // For checking the guardian: crash the first worker after a few seconds.
        if std::env::var_os("SLIME_CRASH_TEST").is_some() && !respawned {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(3));
                panic!("SLIME_CRASH_TEST");
            });
        }
        app(respawned);
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return app(false) };
    let mut crashes: VecDeque<Instant> = VecDeque::new();
    let mut respawned = false;
    loop {
        let mut cmd = Command::new(&exe);
        cmd.arg("--worker");
        if respawned {
            cmd.arg("--respawned");
        }
        let code = match cmd.status() {
            Ok(status) => status.code(),
            // Couldn't start a worker: just run here, unguarded.
            Err(_) => return app(false),
        };
        if !is_crash(code) {
            return;
        }
        log(&format!("worker crashed (exit code {:#x}); restarting", code.unwrap_or(0) as u32));
        let now = Instant::now();
        crashes.push_back(now);
        while crashes.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
            crashes.pop_front();
        }
        if crashes.len() > MAX_RESTARTS {
            log("crashing repeatedly; giving up");
            return;
        }
        respawned = true;
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tells_crashes_from_quits() {
        use super::is_crash;
        assert!(is_crash(Some(0xC000_0409u32 as i32))); // abort after a panic
        assert!(is_crash(Some(0xC000_0005u32 as i32))); // access violation
        assert!(!is_crash(Some(0))); // quit from the menu
        assert!(!is_crash(Some(1))); // ended in Task Manager
        assert!(!is_crash(Some(-1))); // Stop-Process / taskkill /f
        assert!(!is_crash(None));
    }
}
