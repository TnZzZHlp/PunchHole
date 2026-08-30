use std::net::SocketAddrV4;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;

const SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);

/// Builds the three separate script arguments.
pub fn script_arguments(public: SocketAddrV4, local_port: u16) -> Vec<String> {
    vec![
        public.ip().to_string(),
        public.port().to_string(),
        local_port.to_string(),
    ]
}

pub fn run_notification_script(
    script: &std::path::Path,
    arguments: &[String],
    local_port: u16,
) -> Result<(), String> {
    let mut command = Command::new(script);
    command.args(arguments);
    #[cfg(target_os = "linux")]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| {
        format!(
            "mapping local={local_port} notification script {} could not run: {error}",
            script.display()
        )
    })?;

    let deadline = Instant::now() + SCRIPT_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!(
                    "mapping local={local_port} notification script {} failed: {status}",
                    script.display()
                ));
            }
            Ok(None) if Instant::now() >= deadline => {
                terminate_notification_child(&mut child);
                return Err(format!(
                    "mapping local={local_port} notification script {} timed out after {} seconds",
                    script.display(),
                    SCRIPT_TIMEOUT.as_secs()
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                terminate_notification_child(&mut child);
                return Err(format!(
                    "mapping local={local_port} notification script {} status check failed: {error}",
                    script.display()
                ));
            }
        }
    }
}

fn terminate_notification_child(child: &mut Child) {
    #[cfg(target_os = "linux")]
    if !terminate_notification_process_group(child) {
        let _ = child.kill();
    }

    #[cfg(not(target_os = "linux"))]
    let _ = child.kill();

    let _ = child.wait();
}

#[cfg(target_os = "linux")]
fn terminate_notification_process_group(child: &Child) -> bool {
    let pid = match libc::pid_t::try_from(child.id()) {
        Ok(pid) if pid > 0 => pid,
        _ => return false,
    };

    // SAFETY: `pid` is a checked positive child PID, so `-pid` identifies its
    // process group; `kill` does not dereference any Rust pointers.
    unsafe { libc::kill(-pid, libc::SIGKILL) == 0 }
}
