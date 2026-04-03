use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::Config;

/// Lock file guard — removes lock on drop
struct LockGuard {
    path: PathBuf,
}

impl LockGuard {
    fn acquire(data_dir: &Path, kb_name: &str) -> Option<Self> {
        let path = data_dir.join(format!("{}.lock", kb_name));
        if path.exists() {
            // Check if PID is alive
            if let Ok(contents) = fs::read_to_string(&path)
                && let Ok(pid) = contents.trim().parse::<u32>()
            {
                // Check if process is alive (kill with signal 0)
                unsafe {
                    if libc::kill(pid as i32, 0) == 0 {
                        return None; // Process alive, skip
                    }
                }
            }
            // Stale lock, remove it
            let _ = fs::remove_file(&path);
        }
        // Create lock
        let pid = std::process::id();
        if let Ok(mut f) = fs::File::create(&path) {
            let _ = f.write_all(pid.to_string().as_bytes());
            Some(Self { path })
        } else {
            None
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// PID file for daemon mode
fn pid_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("brainjar-watch.pid")
}

/// Stop a running daemon
pub fn stop_daemon(config: &Config) -> Result<()> {
    let data_dir = config.effective_db_dir();
    let pid_path = pid_file_path(&data_dir);
    if !pid_path.exists() {
        println!("No running daemon found");
        return Ok(());
    }
    let pid_str = fs::read_to_string(&pid_path).context("Failed to read PID file")?;
    let pid: i32 = pid_str.trim().parse().context("Invalid PID in file")?;
    unsafe {
        if libc::kill(pid, libc::SIGTERM) == 0 {
            println!("Stopped daemon (PID {})", pid);
        } else {
            println!("Daemon not running (stale PID file)");
        }
    }
    let _ = fs::remove_file(&pid_path);
    Ok(())
}

/// Start daemon mode — re-exec self in background
pub fn start_daemon(config: &Config, interval: u64, kb: Option<&str>, json: bool) -> Result<()> {
    let data_dir = config.effective_db_dir();
    let pid_path = pid_file_path(&data_dir);

    let exe = std::env::current_exe().context("Failed to get current executable")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("watch")
        .arg("--interval")
        .arg(interval.to_string());
    if let Some(kb_name) = kb {
        cmd.arg("--kb").arg(kb_name);
    }
    if json {
        cmd.arg("--json");
    }

    // Redirect output to log file
    let log_path = data_dir.join("brainjar-watch.log");
    let log_file = fs::File::create(&log_path).context("Failed to create log file")?;
    let log_err = log_file.try_clone()?;

    let child = cmd
        .stdout(log_file)
        .stderr(log_err)
        .stdin(std::process::Stdio::null())
        .spawn()
        .context("Failed to spawn daemon")?;

    fs::write(&pid_path, child.id().to_string())?;
    println!("Started daemon (PID {})", child.id());
    println!("Log: {}", log_path.display());
    Ok(())
}

/// Main watch loop
pub async fn run_watch(
    config: &Config,
    kb_name: Option<&str>,
    interval: u64,
    json: bool,
) -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::signal;

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    // Spawn signal handler
    tokio::spawn(async move {
        let _ = signal::ctrl_c().await;
        shutdown_clone.store(true, Ordering::SeqCst);
    });

    let data_dir = config.effective_db_dir();

    // Determine which KBs to watch
    let kb_names: Vec<&str> = if let Some(name) = kb_name {
        if !config.knowledge_bases.contains_key(name) {
            anyhow::bail!("Unknown knowledge base: {}", name);
        }
        vec![name]
    } else {
        config
            .knowledge_bases
            .iter()
            .filter(|(_, kb)| kb.auto_sync)
            .map(|(n, _)| n.as_str())
            .collect()
    };

    if kb_names.is_empty() {
        println!("No knowledge bases to watch (none have auto_sync = true)");
        return Ok(());
    }

    // Startup banner
    if !json {
        let interval_str = if interval >= 60 {
            format!("{}m", interval / 60)
        } else {
            format!("{}s", interval)
        };
        println!(
            "🔭 Watching {} knowledge base{} (interval: {})\n",
            kb_names.len(),
            if kb_names.len() == 1 { "" } else { "s" },
            interval_str
        );
        for name in &kb_names {
            let kb = &config.knowledge_bases[*name];
            let paths = kb.watch_paths.join(", ");
            if let Some(desc) = &kb.description {
                println!("   {}: {} ({})", name, paths, desc);
            } else {
                println!("   {}: {}", name, paths);
            }
        }
        println!();
    }

    // Poll loop
    loop {
        if shutdown.load(Ordering::SeqCst) {
            if !json {
                println!("\n👋 Shutting down watcher");
            }
            break;
        }

        let now = chrono::Local::now().format("%H:%M:%S");

        for name in &kb_names {
            // Try to acquire lock
            let _lock = match LockGuard::acquire(&data_dir, name) {
                Some(guard) => guard,
                None => {
                    if !json {
                        eprintln!("[{}] ⚠ Skipping {}: sync already running", now, name);
                    }
                    continue;
                }
            };

            // Run sync (non-force, non-dry-run)
            match crate::sync::run_sync(config, Some(name), false, false, false, json, false).await {
                Ok(()) => {
                    // run_sync already prints output
                }
                Err(e) => {
                    if !json {
                        eprintln!("[{}] ⚠ {} sync failed: {}", now, name, e);
                    }
                }
            }
        }

        // Sleep in 1-second increments so we can check shutdown flag
        for _ in 0..interval {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_lock_acquire_and_release() {
        let dir = TempDir::new().unwrap();
        let guard = LockGuard::acquire(dir.path(), "test-kb");
        assert!(guard.is_some());
        let lock_path = dir.path().join("test-kb.lock");
        assert!(lock_path.exists());

        // Can't acquire again while held
        let guard2 = LockGuard::acquire(dir.path(), "test-kb");
        assert!(guard2.is_none());

        // Drop releases
        drop(guard);
        assert!(!lock_path.exists());
    }

    #[test]
    fn test_lock_stale_removal() {
        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("test-kb.lock");
        // Write a fake PID that's definitely dead
        fs::write(&lock_path, "999999999").unwrap();

        // Should acquire (stale lock removed)
        let guard = LockGuard::acquire(dir.path(), "test-kb");
        assert!(guard.is_some());
        drop(guard);
    }

    #[test]
    fn test_lock_different_kbs() {
        let dir = TempDir::new().unwrap();
        let guard1 = LockGuard::acquire(dir.path(), "kb-a");
        let guard2 = LockGuard::acquire(dir.path(), "kb-b");
        assert!(guard1.is_some());
        assert!(guard2.is_some());
    }
}
