use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::thread;

use PunchHole::forward_client;

#[cfg(unix)]
#[test]
fn binds_listener_on_connected_socket_local_port() {
    let target_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = match target_listener.local_addr().unwrap() {
        SocketAddr::V4(address) => address,
        SocketAddr::V6(_) => unreachable!(),
    };
    let connected = PunchHole::new_bound_socket(0).unwrap();
    let local_port = connected
        .local_addr()
        .unwrap()
        .as_socket_ipv4()
        .unwrap()
        .port();
    connected.connect(&SocketAddr::V4(target).into()).unwrap();

    let listener = PunchHole::new_bound_socket(local_port).unwrap();
    listener.listen(1).unwrap();
}

#[test]
fn forwards_data_in_both_directions() {
    let target_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = match target_listener.local_addr().unwrap() {
        SocketAddr::V4(address) => address,
        SocketAddr::V6(_) => unreachable!(),
    };
    let target_thread = thread::spawn(move || {
        let (mut stream, _) = target_listener.accept().unwrap();
        let mut request = Vec::new();
        stream.read_to_end(&mut request).unwrap();
        assert_eq!(request, b"ping");
        stream.write_all(b"pong").unwrap();
    });

    let forward_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let forward_addr = forward_listener.local_addr().unwrap();
    let forward_thread = thread::spawn(move || {
        let (client, _) = forward_listener.accept().unwrap();
        forward_client(client, target).unwrap();
    });

    let mut client = TcpStream::connect(forward_addr).unwrap();
    client.write_all(b"ping").unwrap();
    client.shutdown(Shutdown::Write).unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).unwrap();
    assert_eq!(response, b"pong");

    forward_thread.join().unwrap();
    target_thread.join().unwrap();
}
