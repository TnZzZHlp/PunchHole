use std::io;
use std::net::{SocketAddr, SocketAddrV4, TcpStream};
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tracing::{debug, error, info, warn};

use crate::config::{Config, Mapping};
use crate::http::{connect_http, http_keepalive_loop};
use crate::notify::{run_notification_script, script_arguments};
use crate::stun::request_stun;

const RETRY_DELAY: Duration = Duration::from_secs(1);

pub fn run(config: Config) {
    info!(
        mapping_count = config.mappings.len(),
        http = %config.http,
        stun = %config.stun,
        "starting mapping workers"
    );

    let mut workers = Vec::with_capacity(config.mappings.len());
    for (index, mapping) in config.mappings.into_iter().enumerate() {
        let http = config.http;
        let stun = config.stun;
        let mapping_number = index + 1;
        loop {
            let worker_mapping = mapping.clone();
            match thread::Builder::new()
                .name(format!("mapping-{mapping_number}"))
                .spawn(move || {
                    let mut local_port = None;
                    loop {
                        if panic::catch_unwind(AssertUnwindSafe(|| {
                            run_mapping(
                                &worker_mapping,
                                http,
                                stun,
                                &mut local_port,
                                mapping_number,
                            );
                        }))
                        .is_ok()
                        {
                            break;
                        }
                        error!(
                            mapping_number,
                            local_port = ?local_port,
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
                        mapping_number,
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

fn run_mapping(
    mapping: &Mapping,
    http: SocketAddrV4,
    stun: SocketAddrV4,
    local_port: &mut Option<u16>,
    mapping_number: usize,
) {
    let mut last_public = None;
    loop {
        if let Err(error) = run_mapping_once(mapping, http, stun, local_port, &mut last_public) {
            warn!(
                mapping_number,
                local_port = ?local_port,
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
    local_port: &mut Option<u16>,
    last_public: &mut Option<SocketAddrV4>,
) -> io::Result<()> {
    debug!(local_port = ?local_port, http = %http, "starting HTTP setup");
    let http_stream = connect_http(local_port.unwrap_or(0), http)
        .map_err(|error| io::Error::new(error.kind(), format!("HTTP setup failed: {error}")))?;
    let local_port = if let Some(local_port) = *local_port {
        local_port
    } else {
        let assigned = match http_stream.local_addr()? {
            SocketAddr::V4(address) => address.port(),
            SocketAddr::V6(_) => {
                return Err(io::Error::other(
                    "HTTP connection unexpectedly used an IPv6 local address",
                ));
            }
        };
        *local_port = Some(assigned);
        assigned
    };

    debug!(local_port, stun = %stun, "starting STUN setup");
    let public = request_stun(local_port, stun)
        .map_err(|error| io::Error::new(error.kind(), format!("STUN setup failed: {error}")))?;

    if *last_public != Some(public) {
        let arguments = script_arguments(public, local_port);
        run_notification_script(&mapping.script, &arguments, local_port)
            .map_err(|error| io::Error::other(format!("mapping notification failed: {error}")))?;
        *last_public = Some(public);
    }

    maintain_mapping(http_stream, http, local_port, public, &mapping.script)
}

fn maintain_mapping(
    http_stream: TcpStream,
    http: SocketAddrV4,
    local_port: u16,
    public: SocketAddrV4,
    script: &std::path::Path,
) -> io::Result<()> {
    let (stop_sender, stop_receiver) = mpsc::channel();
    let (failure_sender, failure_receiver) = mpsc::channel();
    let keepalive = thread::Builder::new()
        .name(format!("keepalive-{local_port}"))
        .spawn(move || {
            if let Err(error) = http_keepalive_loop(http_stream, http, &stop_receiver) {
                let _ = failure_sender.send(error.to_string());
            }
        })
        .map_err(|error| {
            io::Error::other(format!("could not start HTTP keepalive worker: {error}"))
        })?;

    info!(
        local_port,
        public = %public,
        script = %script.display(),
        "mapping ready"
    );

    let error = failure_receiver.recv().map_or_else(
        |_| "HTTP keepalive stopped".to_string(),
        |error| format!("HTTP keepalive failed: {error}"),
    );
    let result = Err(io::Error::other(error));

    let _ = stop_sender.send(());
    let _ = keepalive.join();
    result
}
