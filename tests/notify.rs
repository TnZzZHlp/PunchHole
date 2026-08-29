use std::path::PathBuf;

#[cfg(unix)]
use PunchHole::NotificationQueue;
use PunchHole::{Mapping, parse_target_endpoint, script_arguments};

#[test]
fn builds_script_arguments_without_combining_values() {
    let mapping = Mapping {
        local_port: 10001,
        target: parse_target_endpoint("192.168.1.20:22").unwrap(),
        script: PathBuf::from("/opt/app one.sh"),
    };

    assert_eq!(
        script_arguments("203.0.113.7:42424".parse().unwrap(), &mapping),
        vec!["203.0.113.7", "42424", "10001", "192.168.1.20", "22"]
    );
}

#[test]
fn uses_public_port_for_dynamic_script_target() {
    let mapping = Mapping {
        local_port: 10001,
        target: parse_target_endpoint("192.168.2.10:0").unwrap(),
        script: PathBuf::from("/opt/qbittorrent-set-port.sh"),
    };

    assert_eq!(
        script_arguments("203.0.113.7:42424".parse().unwrap(), &mapping),
        vec!["203.0.113.7", "42424", "10001", "192.168.2.10", "42424"]
    );
}

#[cfg(unix)]
#[test]
fn retries_failed_notification() {
    use std::fs;
    use std::net::{Ipv4Addr, SocketAddrV4};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    let directory = std::env::temp_dir().join(format!(
        "PunchHole-notify-test-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    let script = directory.join("notify.sh");
    fs::write(
        &script,
        b"#!/bin/sh\ncount_file=\"$0.count\"\ncount=0\nif [ -f \"$count_file\" ]; then count=$(cat \"$count_file\"); fi\ncount=$((count + 1))\nprintf '%s\\n' \"$count\" > \"$count_file\"\n[ \"$count\" -ne 1 ]\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&script, permissions).unwrap();

    let mapping = Mapping {
        local_port: 10001,
        target: PunchHole::Target {
            address: Ipv4Addr::LOCALHOST,
            port: PunchHole::TargetPort::Fixed(22),
        },
        script,
    };
    let queue = NotificationQueue::new(mapping.local_port).unwrap();
    queue
        .send(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 42424), &mapping)
        .unwrap();

    let count_file = directory.join("notify.sh.count");
    let deadline = Instant::now() + Duration::from_secs(4);
    let retried = loop {
        if fs::read_to_string(&count_file).is_ok_and(|count| count.trim() == "2") {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(20));
    };
    drop(queue);
    fs::remove_dir_all(directory).unwrap();
    assert!(retried, "failed notification was not retried");
}

#[cfg(unix)]
#[test]
fn dropping_idle_notification_queue_completes_promptly() {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    let (done_sender, done_receiver) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let queue = NotificationQueue::new(10001).unwrap();
        drop(queue);
        done_sender.send(()).unwrap();
    });

    if done_receiver.recv_timeout(Duration::from_secs(1)).is_ok() {
        worker.join().unwrap();
    } else {
        panic!("dropping an idle notification queue timed out");
    }
}

#[cfg(unix)]
#[test]
fn dropping_notification_queue_joins_running_worker() {
    use std::fs;
    use std::net::{Ipv4Addr, SocketAddrV4};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    let directory = std::env::temp_dir().join(format!(
        "PunchHole-notify-drop-test-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    let script = directory.join("notify.sh");
    fs::write(
        &script,
        b"#!/bin/sh\nprintf 'started\\n' > \"$0.started\"\nsleep 1\nprintf 'done\\n' > \"$0.done\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&script, permissions).unwrap();

    let mapping = Mapping {
        local_port: 10001,
        target: PunchHole::Target {
            address: Ipv4Addr::LOCALHOST,
            port: PunchHole::TargetPort::Fixed(22),
        },
        script,
    };
    let queue = NotificationQueue::new(mapping.local_port).unwrap();
    queue
        .send(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 42424), &mapping)
        .unwrap();

    let started_file = directory.join("notify.sh.started");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !started_file.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(started_file.exists(), "notification script did not start");

    drop(queue);
    assert_eq!(
        fs::read_to_string(directory.join("notify.sh.done"))
            .unwrap()
            .trim(),
        "done"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn coalesces_pending_notifications_to_newest_value() {
    use std::fs;
    use std::net::{Ipv4Addr, SocketAddrV4};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    let directory = std::env::temp_dir().join(format!(
        "PunchHole-notify-coalesce-test-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    let script = directory.join("notify.sh");
    fs::write(
        &script,
        b"#!/bin/sh\nif [ ! -f \"$0.started\" ]; then\n  printf 'started\\n' > \"$0.started\"\n  sleep 1\nfi\nprintf '%s\\n' \"$2\" >> \"$0.log\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&script, permissions).unwrap();

    let mapping = Mapping {
        local_port: 10001,
        target: PunchHole::Target {
            address: Ipv4Addr::LOCALHOST,
            port: PunchHole::TargetPort::Fixed(22),
        },
        script,
    };
    let queue = NotificationQueue::new(mapping.local_port).unwrap();
    queue
        .send(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 42424), &mapping)
        .unwrap();

    let started_file = directory.join("notify.sh.started");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !started_file.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(started_file.exists(), "notification script did not start");

    queue
        .send(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 42425), &mapping)
        .unwrap();
    queue
        .send(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 42426), &mapping)
        .unwrap();

    let log_file = directory.join("notify.sh.log");
    let deadline = Instant::now() + Duration::from_secs(4);
    while !fs::read_to_string(&log_file).is_ok_and(|log| log.lines().count() == 2)
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(20));
    }
    drop(queue);
    assert_eq!(
        fs::read_to_string(log_file)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        vec!["42424", "42426"]
    );
    fs::remove_dir_all(directory).unwrap();
}
