use std::net::SocketAddrV4;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;

use tracing::{error, warn};

use crate::config::Mapping;

const SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);
const NOTIFICATION_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Clone, PartialEq, Eq)]
struct Notification {
    public: SocketAddrV4,
    mapping: Mapping,
}

struct NotificationState {
    pending: Mutex<Option<Notification>>,
    wake: Condvar,
    stop: AtomicBool,
    alive: Arc<AtomicBool>,
}

#[doc(hidden)]
pub struct NotificationQueue {
    state: Arc<NotificationState>,
    worker: Mutex<Option<JoinHandle<()>>>,
    local_port: u16,
}

struct WorkerGuard {
    alive: Arc<AtomicBool>,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

impl Drop for NotificationQueue {
    fn drop(&mut self) {
        {
            let _pending = self
                .state
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.state.stop.store(true, Ordering::Release);
            self.state.wake.notify_all();
        }
        let worker = self
            .worker
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(worker) = worker
            && worker.join().is_err()
        {
            error!(
                local_port = self.local_port,
                "notification worker panicked while stopping"
            );
        }
    }
}

impl NotificationQueue {
    #[doc(hidden)]
    pub fn new(local_port: u16) -> std::io::Result<Self> {
        let state = Arc::new(NotificationState {
            pending: Mutex::new(None),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
            alive: Arc::new(AtomicBool::new(false)),
        });
        let worker = spawn_worker(&state, local_port)?;
        Ok(Self {
            state,
            worker: Mutex::new(Some(worker)),
            local_port,
        })
    }

    fn ensure_worker(&self, worker: &mut Option<JoinHandle<()>>) -> std::io::Result<()> {
        let worker_stopped = worker
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
            || !self.state.alive.load(Ordering::Acquire);
        if !worker_stopped {
            return Ok(());
        }

        if let Some(worker) = worker.take() {
            if worker.join().is_err() {
                error!(
                    local_port = self.local_port,
                    "notification worker panicked; restarting"
                );
            } else {
                warn!(
                    local_port = self.local_port,
                    "notification worker stopped; restarting"
                );
            }
        }

        *worker = Some(spawn_worker(&self.state, self.local_port)?);
        Ok(())
    }

    pub(crate) fn is_alive(&self) -> bool {
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !self.state.stop.load(Ordering::Acquire)
            && self.state.alive.load(Ordering::Acquire)
            && worker.as_ref().is_some_and(|worker| !worker.is_finished())
    }

    #[doc(hidden)]
    pub fn send(&self, public: SocketAddrV4, mapping: &Mapping) -> std::io::Result<()> {
        let mut worker = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.ensure_worker(&mut worker)?;
        if !self.state.alive.load(Ordering::Acquire)
            || worker
                .as_ref()
                .is_none_or(std::thread::JoinHandle::is_finished)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "notification worker is not running",
            ));
        }

        let mut pending = self
            .state
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.state.alive.load(Ordering::Acquire)
            || worker
                .as_ref()
                .is_none_or(std::thread::JoinHandle::is_finished)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "notification worker stopped before notification was queued",
            ));
        }
        *pending = Some(Notification {
            public,
            mapping: mapping.clone(),
        });
        self.state.wake.notify_one();
        if !self.state.alive.load(Ordering::Acquire)
            || worker
                .as_ref()
                .is_none_or(std::thread::JoinHandle::is_finished)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "notification worker stopped after notification was queued",
            ));
        }
        drop(pending);
        drop(worker);
        Ok(())
    }
}

fn spawn_worker(
    state: &Arc<NotificationState>,
    local_port: u16,
) -> std::io::Result<JoinHandle<()>> {
    state.alive.store(true, Ordering::Release);
    let worker_state = state.clone();
    match thread::Builder::new()
        .name(format!("notify-{local_port}"))
        .spawn(move || {
            let _guard = WorkerGuard {
                alive: worker_state.alive.clone(),
            };
            loop {
                let notification = {
                    let mut pending = worker_state
                        .pending
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    loop {
                        if worker_state.stop.load(Ordering::Acquire) {
                            return;
                        }
                        if let Some(notification) = pending.as_ref() {
                            break notification.clone();
                        }
                        pending = worker_state
                            .wake
                            .wait(pending)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                };

                let Notification { public, mapping } = &notification;
                let arguments = script_arguments(*public, mapping);
                match run_notification_script(&mapping.script, &arguments, mapping.local_port) {
                    Ok(()) => {
                        let mut pending = worker_state
                            .pending
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if pending.as_ref() == Some(&notification) {
                            pending.take();
                        }
                    }
                    Err(error) => {
                        error!(
                            local_port = mapping.local_port,
                            public = %public,
                            retry_seconds = NOTIFICATION_RETRY_DELAY.as_secs(),
                            error = %error,
                            "notification script failed; retrying"
                        );
                        let pending = worker_state
                            .pending
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let (_pending, _) = worker_state
                            .wake
                            .wait_timeout_while(pending, NOTIFICATION_RETRY_DELAY, |pending| {
                                !worker_state.stop.load(Ordering::Acquire)
                                    && pending.as_ref() == Some(&notification)
                            })
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                }
            }
        }) {
        Ok(worker) => Ok(worker),
        Err(error) => {
            state.alive.store(false, Ordering::Release);
            Err(std::io::Error::other(format!(
                "could not start notification worker for local port {local_port}: {error}"
            )))
        }
    }
}

/// Builds script arguments as separate values, including the resolved target port.
pub fn script_arguments(public: SocketAddrV4, mapping: &Mapping) -> Vec<String> {
    let target = mapping.target.resolve(public);
    vec![
        public.ip().to_string(),
        public.port().to_string(),
        mapping.local_port.to_string(),
        target.ip().to_string(),
        target.port().to_string(),
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
