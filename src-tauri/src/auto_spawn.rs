//! Auto-spawn the daemon when the tray app starts.

use phoneme_core::Config;
use phoneme_ipc::NamedPipeTransport;
use std::process::Stdio;
use std::time::Duration;

const POLL_TOTAL: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Ensure the daemon is reachable. If not, spawn it detached and poll the
/// pipe for `POLL_TOTAL` before giving up.
pub async fn ensure_running(cfg: &Config) -> anyhow::Result<()> {
    if NamedPipeTransport::connect(&cfg.daemon.pipe_name)
        .await
        .is_ok()
    {
        return Ok(());
    }

    // Spawn detached.
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("phoneme-daemon.exe")))
        .or_else(|| which::which("phoneme-daemon").ok())
        .ok_or_else(|| {
            anyhow::anyhow!("phoneme-daemon not found next to phoneme-tray.exe or on PATH")
        })?;

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new(&exe)
            .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
    }

    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new(&exe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
    }

    // Poll for readiness.
    let start = std::time::Instant::now();
    while start.elapsed() < POLL_TOTAL {
        if NamedPipeTransport::connect(&cfg.daemon.pipe_name)
            .await
            .is_ok()
        {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!("daemon did not come up within {:?}", POLL_TOTAL)
}
