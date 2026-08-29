use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tracing::{debug, error, info, warn};

use crate::config::{Config, Mapping};
use crate::http::{connect_http, http_keepalive_loop};
use crate::net::{self, CONNECT_TIMEOUT};
use crate::notify::{NotificationQueue, run_notification_script, script_arguments};
use crate::stun::request_stun;

const FORWARD_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const FORWARD_READ_POLL: Duration = Duration::from_secs(1);
const ACCEPT_POLL: Duration = Duration::from_millis(100);
const RETRY_DELAY: Duration = Duration::from_secs(1);
// ponytail: fixed 64-client cap; make it configurable only when device capacity requires it.
const MAX_CLIENTS: usize = 64;

pub fn run(config: Config) {
    info!(
        mapping_count = config.mappings.len(),
        http = %config.http,
        stun = %config.stun,
        "starting mapping workers"
    );

    let mut workers = Vec::with_capacity(config.mappings.len());
    for mapping in config.mappings {
        let http = config.http;
        let stun = config.stun;
        let local_port = mapping.local_port;
        loop {
            let worker_mapping = mapping.clone();
            match thread::Builder::new()
                .name(format!("mapping-{local_port}"))
                .spawn(move || {
                    loop {
                        if panic::catch_unwind(AssertUnwindSafe(|| {
                            run_mapping(&worker_mapping, http, stun);
                        }))
                        .is_ok()
                        {
                            break;
                        }
                        error!(
                            local_port,
                            retry_seconds = RETRY_DELAY.as_secs(),
                            "mapping worker panicked; restarting"
                        );
                        thread::sleep(RETRY_DELAY);
                    }
                }) {
                Ok(worker) => {
                    workers.push(worker);
                    break;
                }
                Err(error) => {
                    error!(
                        local_port,
                        error = %error,
                        retry_seconds = RETRY_DELAY.as_secs(),
                        "could not start mapping worker; retrying"
                    );
                    thread::sleep(RETRY_DELAY);
                }
            }
        }
    }

    for worker in workers {
        if worker.join().is_err() {
            error!("mapping worker terminated unexpectedly");
        }
    }
}

fn run_mapping(mapping: &Mapping, http: SocketAddrV4, stun: SocketAddrV4) {
    let mut last_public = None;
    let active_clients = Arc::new(AtomicUsize::new(0));
    let notifications = if mapping.target.uses_public_port() {
        None
    } else {
        loop {
            match NotificationQueue::new(mapping.local_port) {
                Ok(queue) => break Some(queue),
                Err(error) => {
                    warn!(
                        local_port = mapping.local_port,
                        error = %error,
                        retry_seconds = RETRY_DELAY.as_secs(),
                        "could not start notification worker; retrying"
                    );
                    thread::sleep(RETRY_DELAY);
                }
            }
        }
    };

    loop {
        if let Err(error) = run_mapping_once(
            mapping,
            http,
            stun,
            &mut last_public,
            &active_clients,
            notifications.as_ref(),
        ) {
            warn!(
                local_port = mapping.local_port,
                error = %error,
                retry_seconds = RETRY_DELAY.as_secs(),
                "mapping stopped; retrying"
            );
        }
        thread::sleep(RETRY_DELAY);
    }
}

fn run_mapping_once(
    mapping: &Mapping,
    http: SocketAddrV4,
    stun: SocketAddrV4,
    last_public: &mut Option<SocketAddrV4>,
    active_clients: &Arc<AtomicUsize>,
    notifications: Option<&NotificationQueue>,
) -> io::Result<()> {
    debug!(
        local_port = mapping.local_port,
        http = %http,
        "starting HTTP setup"
    );
    let http_stream = connect_http(mapping.local_port, http)
        .map_err(|error| io::Error::new(error.kind(), format!("HTTP setup failed: {error}")))?;
    debug!(
        local_port = mapping.local_port,
        stun = %stun,
        "starting STUN setup"
    );
    let public = request_stun(mapping.local_port, stun)
        .map_err(|error| io::Error::new(error.kind(), format!("STUN setup failed: {error}")))?;

    let target = mapping.target.resolve(public);
    let notification_needed = *last_public != Some(public)
        || notifications.is_some_and(|notifications| !notifications.is_alive());
    if notification_needed {
        if mapping.target.uses_public_port() {
            let arguments = script_arguments(public, mapping);
            run_notification_script(&mapping.script, &arguments, mapping.local_port).map_err(
                |error| io::Error::other(format!("mapping notification failed: {error}")),
            )?;
        } else if let Some(notifications) = notifications {
            notifications
                .send(public, mapping)
                .map_err(|error| io::Error::other(format!("notification queue failed: {error}")))?;
        }
        *last_public = Some(public);
    }
    let listener = bind_listener(mapping.local_port)
        .map_err(|error| io::Error::new(error.kind(), format!("listener setup failed: {error}")))?;
    serve_listener(
        &listener,
        http_stream,
        ListenerContext {
            http,
            local_port: mapping.local_port,
            public,
            target,
            active_clients,
            notifications,
        },
    )
}

fn bind_listener(local_port: u16) -> io::Result<TcpListener> {
    let socket = net::new_bound_socket(local_port)?;
    socket.listen(128)?;
    let listener: TcpListener = socket.into();
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn try_acquire_client_slot(active_clients: &AtomicUsize) -> bool {
    let mut current = active_clients.load(Ordering::Acquire);
    loop {
        if current >= MAX_CLIENTS {
            return false;
        }
        match active_clients.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(next) => current = next,
        }
    }
}

struct ActiveClientSlot {
    active_clients: Arc<AtomicUsize>,
}

impl ActiveClientSlot {
    const fn new(active_clients: Arc<AtomicUsize>) -> Self {
        Self { active_clients }
    }
}

impl Drop for ActiveClientSlot {
    fn drop(&mut self) {
        self.active_clients.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy)]
struct ListenerContext<'a> {
    http: SocketAddrV4,
    local_port: u16,
    public: SocketAddrV4,
    target: SocketAddrV4,
    active_clients: &'a Arc<AtomicUsize>,
    notifications: Option<&'a NotificationQueue>,
}

fn serve_listener(
    listener: &TcpListener,
    http_stream: TcpStream,
    context: ListenerContext<'_>,
) -> io::Result<()> {
    let ListenerContext {
        http,
        local_port,
        public,
        target,
        active_clients,
        notifications,
    } = context;
    let (stop_sender, stop_receiver) = mpsc::channel();
    let (failure_sender, failure_receiver) = mpsc::channel();
    let keepalive = match thread::Builder::new()
        .name(format!("keepalive-{local_port}"))
        .spawn(move || {
            if let Err(error) = http_keepalive_loop(http_stream, http, &stop_receiver) {
                let _ = failure_sender.send(error.to_string());
            }
        }) {
        Ok(worker) => {
            info!(
                local_port,
                public = %public,
                target = %target,
                "mapping listener ready"
            );
            worker
        }
        Err(error) => {
            return Err(io::Error::other(format!(
                "could not start HTTP keepalive worker: {error}"
            )));
        }
    };

    let result = loop {
        if notifications.is_some_and(|notifications| !notifications.is_alive()) {
            break Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "notification worker stopped",
            ));
        }

        match failure_receiver.try_recv() {
            Ok(error) => {
                break Err(io::Error::other(format!("HTTP keepalive failed: {error}")));
            }
            Err(TryRecvError::Disconnected) => {
                break Err(io::Error::other("HTTP keepalive stopped"));
            }
            Err(TryRecvError::Empty) => {}
        }

        match listener.accept() {
            Ok((client, peer)) => {
                debug!(local_port, peer = %peer, "client accepted");
                if !try_acquire_client_slot(active_clients) {
                    warn!(
                        local_port,
                        peer = %peer,
                        max_clients = MAX_CLIENTS,
                        "rejecting client: active client limit reached"
                    );
                    continue;
                }

                let slot = ActiveClientSlot::new(Arc::clone(active_clients));
                let worker = thread::Builder::new()
                    .name(format!("forward-{local_port}"))
                    .spawn(move || {
                        let _slot = slot;
                        if let Err(error) = forward_client(client, target) {
                            error!(
                                local_port,
                                target = %target,
                                error = %error,
                                "forwarding failed"
                            );
                        }
                    });
                if let Err(error) = worker {
                    error!(
                        local_port,
                        peer = %peer,
                        error = %error,
                        "could not start forwarding worker"
                    );
                }
            }
            Err(error) if net::is_retryable_accept_error(&error) => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(error) => break Err(error),
        }
    };

    let _ = stop_sender.send(());
    let _ = keepalive.join();
    result
}

struct ForwardActivity {
    last: Mutex<Instant>,
}

impl ForwardActivity {
    fn new() -> Self {
        Self {
            last: Mutex::new(Instant::now()),
        }
    }

    fn touch(&self) {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *last = Instant::now();
    }

    fn is_idle(&self) -> bool {
        let last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        last.elapsed() >= FORWARD_IDLE_TIMEOUT
    }
}

fn copy_with_activity(
    reader: &mut TcpStream,
    writer: &mut TcpStream,
    activity: &ForwardActivity,
) -> io::Result<u64> {
    let mut buffer = [0; 8192];
    let mut copied = 0;

    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(copied),
            Ok(length) => {
                activity.touch();
                writer.write_all(&buffer[..length])?;
                activity.touch();
                copied += length as u64;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                ) =>
            {
                if activity.is_idle() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "forwarding connection idle timeout",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
}

/// Forwards a connected client stream to an IPv4 target in both directions.
pub fn forward_client(client: TcpStream, target: SocketAddrV4) -> io::Result<()> {
    let target_addr = SocketAddr::V4(target);
    let target_stream = TcpStream::connect_timeout(&target_addr, CONNECT_TIMEOUT)?;
    target_stream.set_nodelay(true)?;

    let mut client_read = client.try_clone()?;
    let mut target_write = target_stream.try_clone()?;
    let mut target_read = target_stream.try_clone()?;
    let mut client_write = client.try_clone()?;
    let client_shutdown = client.try_clone()?;
    let target_shutdown = target_stream.try_clone()?;
    client_read.set_read_timeout(Some(FORWARD_READ_POLL))?;
    target_read.set_read_timeout(Some(FORWARD_READ_POLL))?;
    target_write.set_write_timeout(Some(FORWARD_IDLE_TIMEOUT))?;
    client_write.set_write_timeout(Some(FORWARD_IDLE_TIMEOUT))?;
    drop(client);
    drop(target_stream);

    let activity = Arc::new(ForwardActivity::new());
    let upstream_activity = activity.clone();
    let upstream = match thread::Builder::new()
        .name("forward-upstream".to_string())
        .spawn(move || {
            let result =
                copy_with_activity(&mut client_read, &mut target_write, &upstream_activity);
            if result.is_err() {
                let _ = client_read.shutdown(Shutdown::Both);
                let _ = target_write.shutdown(Shutdown::Both);
            } else {
                let _ = target_write.shutdown(Shutdown::Write);
            }
            result
        }) {
        Ok(worker) => worker,
        Err(error) => {
            let _ = client_shutdown.shutdown(Shutdown::Both);
            let _ = target_shutdown.shutdown(Shutdown::Both);
            return Err(io::Error::other(format!(
                "could not start upstream forwarding worker: {error}"
            )));
        }
    };
    let downstream_activity = activity;
    let downstream = match thread::Builder::new()
        .name("forward-downstream".to_string())
        .spawn(move || {
            let result =
                copy_with_activity(&mut target_read, &mut client_write, &downstream_activity);
            if result.is_err() {
                let _ = client_write.shutdown(Shutdown::Both);
                let _ = target_read.shutdown(Shutdown::Both);
            } else {
                let _ = client_write.shutdown(Shutdown::Write);
            }
            result
        }) {
        Ok(worker) => worker,
        Err(error) => {
            let _ = client_shutdown.shutdown(Shutdown::Both);
            let _ = target_shutdown.shutdown(Shutdown::Both);
            let _ = upstream.join();
            return Err(io::Error::other(format!(
                "could not start downstream forwarding worker: {error}"
            )));
        }
    };

    let upstream_result = upstream.join().unwrap_or_else(|_| {
        let _ = client_shutdown.shutdown(Shutdown::Both);
        let _ = target_shutdown.shutdown(Shutdown::Both);
        Err(io::Error::other("upstream forwarding worker panicked"))
    });
    if upstream_result.is_err() {
        let _ = client_shutdown.shutdown(Shutdown::Both);
        let _ = target_shutdown.shutdown(Shutdown::Both);
    }

    let downstream_result = downstream.join().unwrap_or_else(|_| {
        let _ = client_shutdown.shutdown(Shutdown::Both);
        let _ = target_shutdown.shutdown(Shutdown::Both);
        Err(io::Error::other("downstream forwarding worker panicked"))
    });
    upstream_result?;
    downstream_result?;
    Ok(())
}
