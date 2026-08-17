use anyhow::{Context, Result};
use job_hunter_core::paths::Paths;
use std::process::Command;

pub fn run(now: bool) -> Result<()> {
    let paths = Paths::resolve()?;
    let exe = std::env::current_exe().context("cannot resolve the job-hunter binary path")?;
    let unit = format!(
        "[Unit]\n\
         Description=Job Hunter ingest server\n\n\
         [Service]\n\
         ExecStart={} serve\n\
         Restart=on-failure\n\n\
         [Install]\n\
         WantedBy=default.target\n",
        exe.display()
    );
    let unit_path = &paths.systemd_unit_file;
    if let Some(parent) = unit_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(unit_path, unit)?;
    println!("Wrote {}", unit_path.display());
    println!("Note: after upgrading the binary, restart with `systemctl --user restart job-hunter`.");

    if now {
        for args in [
            vec!["--user", "daemon-reload"],
            vec!["--user", "enable", "--now", "job-hunter.service"],
        ] {
            let display = format!("systemctl {}", args.join(" "));
            let status = Command::new("systemctl")
                .args(&args)
                .status()
                .context("cannot run systemctl - is this a systemd system?")?;
            if !status.success() {
                anyhow::bail!("`{display}` failed with {status}");
            }
            println!("ran {display}");
        }
        println!("Service is enabled and running. Logs: journalctl --user -u job-hunter -f");
    } else {
        println!("\nTo enable and start it:");
        println!("  systemctl --user daemon-reload");
        println!("  systemctl --user enable --now job-hunter.service");
    }
    Ok(())
}
