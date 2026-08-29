use std::io::{self, Write};
use std::net::{SocketAddr, SocketAddrV4, TcpListener};
use std::thread;

use PunchHole::{connect_http, validate_http_response};

#[test]
fn requires_persistent_http_responses() {
    assert!(validate_http_response(b"HTTP/1.1 200 OK\r\n\r\n").is_ok());
    assert!(validate_http_response(b"HTTP/1.1 404 Not Found\r\n\r\n").is_ok());
    assert!(validate_http_response(b"HTTP/1.1 500 Server Error\r\n\r\n").is_ok());
    assert!(validate_http_response(b"HTTP/1.1 599 Error\r\n\r\n").is_ok());
    assert!(validate_http_response(b"HTTP/1.1 100 Continue\r\n\r\n").is_err());
    assert!(validate_http_response(b"HTTP/1.1 600 Error\r\n\r\n").is_err());
    assert!(validate_http_response(b"HTTP/1.1 200 OK\r\nConnection: CLOSE\r\n\r\n").is_err());
    assert!(
        validate_http_response(b"HTTP/1.1 200 OK\r\nConnection: keep-alive, close\r\n\r\n")
            .is_err()
    );
    assert!(
        validate_http_response(b"HTTP/1.0 200 OK\r\nConnection: keep-alive, close\r\n\r\n")
            .is_err()
    );
    assert!(validate_http_response(b"HTTP/1.0 200 OK\r\n\r\n").is_err());
    assert!(
        validate_http_response(b"HTTP/1.0 404 Not Found\r\nConnection: keep-alive\r\n\r\n").is_ok()
    );
    assert!(validate_http_response(b"HTTP/1.1 0200 OK\r\n\r\n").is_err());
    assert!(validate_http_response(b"HTTP/1.0 200 OK\r\nconnection: Keep-Alive\r\n\r\n").is_ok());
}

#[test]
fn rejects_malformed_status_line_separators() {
    assert!(validate_http_response(b" HTTP/1.1 200 OK\r\n\r\n").is_err());
    assert!(validate_http_response(b"HTTP/1.1\t200 OK\r\n\r\n").is_err());
    assert!(validate_http_response(b"HTTP/1.1 200\tOK\r\n\r\n").is_err());
    assert!(validate_http_response(b"HTTP/1.1  200 OK\r\n\r\n").is_err());
    assert!(validate_http_response(b"HTTP/1.1 200\r\n\r\n").is_err());
}

#[test]
fn rejects_invalid_header_field_names() {
    assert!(validate_http_response(b"HTTP/1.1 200 OK\r\nBad Header: value\r\n\r\n").is_err());
    assert!(validate_http_response(b"HTTP/1.1 200 OK\r\n Header: value\r\n\r\n").is_err());
    assert!(validate_http_response(b"HTTP/1.1 200 OK\r\nHeader : value\r\n\r\n").is_err());
    assert!(validate_http_response(b"HTTP/1.1 200 OK\r\nGood-Header: value\r\n\r\n").is_ok());
}

#[test]
fn rejects_control_bytes_in_header_field_values() {
    assert!(validate_http_response(b"HTTP/1.1 200 OK\r\nConnection: close\x01\r\n\r\n").is_err());
    assert!(validate_http_response(b"HTTP/1.1 200 OK\r\nHeader: value\x7f\r\n\r\n").is_err());
    assert!(validate_http_response(b"HTTP/1.1 200 OK\r\nHeader: value\tmore\r\n\r\n").is_ok());
}

#[test]
fn accepts_horizontal_tab_in_reason_phrase() {
    assert!(validate_http_response(b"HTTP/1.1 200 OK\tready\r\n\r\n").is_ok());
}

#[test]
fn rejects_bare_header_line_endings() {
    assert!(validate_http_response(b"HTTP/1.1 200 OK\nHeader: value\r\n\r\n").is_err());
    assert!(
        validate_http_response(b"HTTP/1.1 200 OK\r\nHeader: value\rHeader-2: value\r\n\r\n")
            .is_err()
    );
}

#[test]
fn rejects_invalid_connection_options() {
    assert!(validate_http_response(b"HTTP/1.1 200 OK\r\nConnection: close;foo\r\n\r\n").is_err());
    assert!(
        validate_http_response(b"HTTP/1.1 200 OK\r\nConnection: close,,keep-alive\r\n\r\n")
            .is_err()
    );
    assert!(validate_http_response(b"HTTP/1.1 200 OK\r\nConnection:\r\n\r\n").is_err());
}

fn ipv4_address(listener: &TcpListener) -> SocketAddrV4 {
    match listener.local_addr().unwrap() {
        SocketAddr::V4(address) => address,
        SocketAddr::V6(_) => unreachable!(),
    }
}

#[test]
fn consumes_informational_response_before_final_response() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = ipv4_address(&listener);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .write_all(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\n\r\n")
            .unwrap();
    });

    drop(connect_http(0, address).unwrap());
    server.join().unwrap();
}

#[test]
fn rejects_switching_protocols_in_stream_path() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = ipv4_address(&listener);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n")
            .unwrap();
    });

    let error = connect_http(0, address).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("101"));
    server.join().unwrap();
}
