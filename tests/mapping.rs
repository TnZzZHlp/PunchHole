use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

use PunchHole::{
    STUN_BINDING_SUCCESS, STUN_MAGIC_COOKIE, StunConnection, XOR_MAPPED_ADDRESS,
    http_keepalive_loop,
};

fn ipv4_address(listener: &TcpListener) -> SocketAddrV4 {
    match listener.local_addr().unwrap() {
        SocketAddr::V4(address) => address,
        SocketAddr::V6(_) => unreachable!(),
    }
}

fn read_stun_request(stream: &mut TcpStream) -> [u8; 12] {
    let mut request = [0; 20];
    stream.read_exact(&mut request).unwrap();
    assert_eq!(&request[..2], &0x0001_u16.to_be_bytes());
    assert_eq!(&request[2..4], &0_u16.to_be_bytes());
    assert_eq!(&request[4..8], &STUN_MAGIC_COOKIE.to_be_bytes());
    request[8..].try_into().unwrap()
}

fn stun_success_response(transaction_id: [u8; 12], public: SocketAddrV4) -> [u8; 32] {
    let mut response = [0; 32];
    response[..2].copy_from_slice(&STUN_BINDING_SUCCESS.to_be_bytes());
    response[2..4].copy_from_slice(&12_u16.to_be_bytes());
    response[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    response[8..20].copy_from_slice(&transaction_id);
    response[20..22].copy_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
    response[22..24].copy_from_slice(&8_u16.to_be_bytes());
    response[25] = 1;
    response[26..28]
        .copy_from_slice(&(public.port() ^ (STUN_MAGIC_COOKIE >> 16) as u16).to_be_bytes());
    response[28..32].copy_from_slice(
        &(u32::from_be_bytes(public.ip().octets()) ^ STUN_MAGIC_COOKIE).to_be_bytes(),
    );
    response
}

#[test]
fn reuses_one_tcp_connection_for_repeated_stun_requests() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = ipv4_address(&listener);
    let first: SocketAddrV4 = "198.51.100.1:41000".parse().unwrap();
    let second: SocketAddrV4 = "198.51.100.2:41001".parse().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        for public in [first, second] {
            let transaction_id = read_stun_request(&mut stream);
            stream
                .write_all(&stun_success_response(transaction_id, public))
                .unwrap();
        }
    });

    let mut stun = StunConnection::connect(0, address).unwrap();
    assert_eq!(stun.request().unwrap(), first);
    assert_eq!(stun.request().unwrap(), second);
    server.join().unwrap();
}

#[test]
fn keepalive_runs_periodic_check_and_propagates_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = ipv4_address(&listener);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut request = Vec::new();
        loop {
            let mut byte = [0; 1];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
    });

    let stream = TcpStream::connect(address).unwrap();
    let shutdown_stream = stream.try_clone().unwrap();
    let checks = Arc::new(AtomicUsize::new(0));
    let checks_for_keepalive = Arc::clone(&checks);
    let (result_sender, result_receiver) = mpsc::channel();
    let keepalive = thread::spawn(move || {
        let result = http_keepalive_loop(stream, address, Duration::from_millis(1), || {
            checks_for_keepalive.fetch_add(1, Ordering::Relaxed);
            Err(io::Error::other("periodic check failed"))
        });
        result_sender.send(result).unwrap();
    });

    let result = match result_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(result) => result,
        Err(error) => {
            let _ = shutdown_stream.shutdown(Shutdown::Both);
            let _ = keepalive.join();
            let _ = server.join();
            panic!("keepalive test did not complete within 2 seconds: {error}");
        }
    };
    let keepalive_result = keepalive.join();
    let server_result = server.join();
    keepalive_result.unwrap();
    server_result.unwrap();

    let error = result.unwrap_err();
    assert_eq!(checks.load(Ordering::Relaxed), 1);
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert!(error.to_string().contains("periodic check failed"));
}

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::net::{Ipv4Addr, SocketAddrV4};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use PunchHole::notify_if_public_changed;

    fn temporary_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("punchhole-{label}-{}-{nonce}", std::process::id()))
    }

    fn make_executable(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    fn reruns_notification_only_for_changed_public_endpoint() {
        let script = temporary_path("mapping-script");
        let log = temporary_path("mapping-log");
        make_executable(
            &script,
            &format!(
                "#!/bin/sh\nprintf '%s %s %s\\n' \"$1\" \"$2\" \"$3\" >> '{}'\n",
                log.display()
            ),
        );

        let first = SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, 1), 41_000);
        let changed_ip = SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, 2), 41_000);
        let changed_port = SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, 2), 41_001);
        let mut last_public = None;

        notify_if_public_changed(&script, 12_000, first, &mut last_public).unwrap();
        notify_if_public_changed(&script, 12_000, first, &mut last_public).unwrap();
        notify_if_public_changed(&script, 12_000, changed_ip, &mut last_public).unwrap();
        notify_if_public_changed(&script, 12_000, changed_port, &mut last_public).unwrap();

        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            "198.51.100.1 41000 12000\n198.51.100.2 41000 12000\n198.51.100.2 41001 12000\n"
        );
        assert_eq!(last_public, Some(changed_port));

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(log);
    }

    #[test]
    fn retries_endpoint_change_after_notification_failure() {
        let script = temporary_path("recovering-script");
        let marker = temporary_path("recovering-marker");
        let log = temporary_path("recovering-log");
        make_executable(
            &script,
            &format!(
                "#!/bin/sh\nif [ ! -e '{}' ]; then\n  : > '{}'\n  exit 1\nfi\nprintf '%s %s %s\\n' \"$1\" \"$2\" \"$3\" >> '{}'\n",
                marker.display(),
                marker.display(),
                log.display()
            ),
        );

        let previous = SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, 1), 41_000);
        let changed = SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, 2), 41_001);
        let mut last_public = Some(previous);

        assert!(notify_if_public_changed(&script, 12_000, changed, &mut last_public).is_err());
        assert_eq!(last_public, Some(previous));
        notify_if_public_changed(&script, 12_000, changed, &mut last_public).unwrap();
        assert_eq!(last_public, Some(changed));
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            "198.51.100.2 41001 12000\n"
        );

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(marker);
        let _ = fs::remove_file(log);
    }
}
