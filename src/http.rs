use std::io::{self, Read, Write};
use std::net::{SocketAddrV4, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use crate::net::connect_from_local;

pub const HTTP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
pub const HTTP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
pub const HTTP_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HTTP_HEADERS: usize = 64 * 1024;

fn send_http_head(stream: &mut TcpStream, http: SocketAddrV4) -> io::Result<()> {
    let request = format!("HEAD / HTTP/1.1\r\nHost: {http}\r\nConnection: keep-alive\r\n\r\n");
    stream.write_all(request.as_bytes())?;
    stream.flush()
}

fn trim_ascii(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(start, |index| index + 1);
    &value[start..end]
}

const fn is_http_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

const fn is_http_field_value_byte(byte: u8) -> bool {
    matches!(byte, b'\t' | b' ' | 0x21..=0x7e | 0x80..=0xff)
}

fn parse_status_line(line: &[u8]) -> io::Result<(&'static str, u16)> {
    let (version, version_length) = if line.starts_with(b"HTTP/1.0") {
        ("HTTP/1.0", b"HTTP/1.0".len())
    } else if line.starts_with(b"HTTP/1.1") {
        ("HTTP/1.1", b"HTTP/1.1".len())
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP response version is unsupported",
        ));
    };

    if line.get(version_length) != Some(&b' ') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP status line separator is invalid",
        ));
    }
    let status_start = version_length + 1;
    let status_end = status_start + 3;
    if line.len() <= status_end
        || !line[status_start..status_end]
            .iter()
            .all(u8::is_ascii_digit)
        || line[status_end] != b' '
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP status is invalid",
        ));
    }
    let status = std::str::from_utf8(&line[status_start..status_end])
        .expect("HTTP status contains only ASCII digits")
        .parse::<u16>()
        .expect("three ASCII digits always fit in a u16");
    if !(100..=599).contains(&status) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("HTTP response returned status {status}"),
        ));
    }
    if line[status_end + 1..].iter().any(|byte| {
        !(*byte == b'\t' || *byte == b' ' || (0x21..=0x7e).contains(byte) || *byte >= 0x80)
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP reason phrase is invalid",
        ));
    }

    Ok((version, status))
}

fn validate_http_response_block(headers: &[u8], allow_informational: bool) -> io::Result<u16> {
    let header_end = headers
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP header terminator is missing",
            )
        })?;
    let header_block = &headers[..header_end];
    for (index, byte) in header_block.iter().enumerate() {
        let valid_line_ending = match byte {
            b'\r' => header_block.get(index + 1) == Some(&b'\n'),
            b'\n' => index > 0 && header_block[index - 1] == b'\r',
            _ => true,
        };
        if !valid_line_ending {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP header line ending is invalid",
            ));
        }
    }

    let mut lines = header_block.split(|byte| *byte == b'\n');
    let status_line = lines
        .next()
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "HTTP status line is missing"))?;
    let (version, status) = parse_status_line(status_line)?;
    if !((200..=599).contains(&status) || allow_informational && (100..=199).contains(&status)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("HTTP response returned non-final status {status}"),
        ));
    }

    let mut connection_close = false;
    let mut connection_keep_alive = false;
    for line in lines {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let colon = line.iter().position(|byte| *byte == b':').ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP header field is missing a colon",
            )
        })?;
        let name = &line[..colon];
        if name.is_empty() || !name.iter().copied().all(is_http_token_byte) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP header field name is invalid",
            ));
        }
        let raw_value = &line[colon + 1..];
        if !raw_value.iter().copied().all(is_http_field_value_byte) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP header field value is invalid",
            ));
        }
        let value = trim_ascii(raw_value);
        if name.eq_ignore_ascii_case(b"connection") {
            for token in value.split(|byte| *byte == b',').map(trim_ascii) {
                if token.is_empty() || !token.iter().copied().all(is_http_token_byte) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "HTTP Connection option is invalid",
                    ));
                }
                connection_close |= token.eq_ignore_ascii_case(b"close");
                connection_keep_alive |= token.eq_ignore_ascii_case(b"keep-alive");
            }
        }
    }

    if (100..=199).contains(&status) {
        return Ok(status);
    }
    let persistent = match version {
        "HTTP/1.1" => !connection_close,
        "HTTP/1.0" => connection_keep_alive && !connection_close,
        _ => false,
    };
    if !persistent {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP response is not persistent",
        ));
    }
    Ok(status)
}

/// Validates an HTTP response status and the connection semantics needed by the keepalive loop.
pub fn validate_http_response(headers: &[u8]) -> io::Result<()> {
    validate_http_response_block(headers, false).map(|_| ())
}

fn read_http_response_headers(stream: &mut TcpStream) -> io::Result<()> {
    let deadline = Instant::now() + HTTP_RESPONSE_TIMEOUT;
    let mut headers = Vec::with_capacity(1024);

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "HTTP response deadline exceeded",
            ));
        }
        stream.set_read_timeout(Some(remaining))?;

        let mut byte = [0; 1];
        let length = stream.read(&mut byte)?;
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "HTTP response deadline exceeded",
            ));
        }
        if length == 0 {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "HTTP peer closed",
            ));
        }
        headers.push(byte[0]);
        if headers.len() > MAX_HTTP_HEADERS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP response headers are too large",
            ));
        }
        if headers.ends_with(b"\r\n\r\n") {
            let status = validate_http_response_block(&headers, true)?;
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "HTTP response deadline exceeded",
                ));
            }
            if status == 101 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "HTTP 101 Switching Protocols is unsupported",
                ));
            }
            if (100..=199).contains(&status) {
                headers.clear();
                continue;
            }
            stream.set_read_timeout(Some(HTTP_RESPONSE_TIMEOUT))?;
            return Ok(());
        }
    }
}

#[doc(hidden)]
pub fn connect_http(local_port: u16, http: SocketAddrV4) -> io::Result<TcpStream> {
    let mut stream = connect_from_local(local_port, http)?;
    stream.set_read_timeout(Some(HTTP_RESPONSE_TIMEOUT))?;
    stream.set_write_timeout(Some(HTTP_WRITE_TIMEOUT))?;
    send_http_head(&mut stream, http)?;
    read_http_response_headers(&mut stream)?;
    Ok(stream)
}

pub fn http_keepalive_loop<F>(
    mut stream: TcpStream,
    http: SocketAddrV4,
    interval: Duration,
    mut periodic_check: F,
) -> io::Result<()>
where
    F: FnMut() -> io::Result<()>,
{
    stream.set_read_timeout(Some(HTTP_RESPONSE_TIMEOUT))?;
    stream.set_write_timeout(Some(HTTP_WRITE_TIMEOUT))?;

    loop {
        thread::sleep(interval);
        send_http_head(&mut stream, http)?;
        read_http_response_headers(&mut stream)?;
        periodic_check()?;
    }
}
