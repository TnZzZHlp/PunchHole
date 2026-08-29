use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::net::connect_from_local;

pub const STUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;
pub const STUN_BINDING_SUCCESS: u16 = 0x0101;
pub const XOR_MAPPED_ADDRESS: u16 = 0x0020;

pub fn request_stun(local_port: u16, stun: SocketAddrV4) -> io::Result<SocketAddrV4> {
    let mut stream = connect_from_local(local_port, stun)?;
    stream.set_read_timeout(Some(STUN_TIMEOUT))?;
    stream.set_write_timeout(Some(STUN_TIMEOUT))?;
    let transaction_id = next_transaction_id();
    stream.write_all(&stun_binding_request(transaction_id))?;
    stream.flush()?;
    read_stun_response(&mut stream, transaction_id)
}

fn next_transaction_id() -> [u8; 12] {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let timestamp = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    )
    .unwrap_or_default();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut transaction_id = [0; 12];
    transaction_id[..8].copy_from_slice(&timestamp.to_be_bytes());
    transaction_id[8..].copy_from_slice(&counter.to_be_bytes()[4..]);
    transaction_id
}

fn stun_binding_request(transaction_id: [u8; 12]) -> [u8; 20] {
    let mut request = [0; 20];
    request[0..2].copy_from_slice(&0x0001_u16.to_be_bytes());
    request[2..4].copy_from_slice(&0_u16.to_be_bytes());
    request[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    request[8..].copy_from_slice(&transaction_id);
    request
}

fn read_stun_response(
    stream: &mut TcpStream,
    transaction_id: [u8; 12],
) -> io::Result<SocketAddrV4> {
    let deadline = std::time::Instant::now() + STUN_TIMEOUT;
    let mut header = [0; 20];
    read_exact_until(stream, &mut header, deadline)?;
    let body_length = usize::from(u16::from_be_bytes([header[2], header[3]]));
    let mut response = Vec::with_capacity(20 + body_length);
    response.extend_from_slice(&header);
    response.resize(20 + body_length, 0);
    read_exact_until(stream, &mut response[20..], deadline)?;
    parse_xor_mapped_address(&response, transaction_id)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn read_exact_until(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    deadline: std::time::Instant,
) -> io::Result<()> {
    let mut offset = 0;
    while offset < buffer.len() {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "STUN response read deadline elapsed",
            ));
        }
        stream.set_read_timeout(Some(remaining))?;
        match stream.read(&mut buffer[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "failed to fill whole buffer",
                ));
            }
            Ok(bytes_read) => {
                offset += bytes_read;
                if std::time::Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "STUN response read deadline elapsed",
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Strictly parses an IPv4 XOR-MAPPED-ADDRESS from a STUN Binding response.
pub fn parse_xor_mapped_address(
    packet: &[u8],
    transaction_id: [u8; 12],
) -> Result<SocketAddrV4, String> {
    if packet.len() < 20 {
        return Err("STUN packet is shorter than its header".to_string());
    }
    if u16::from_be_bytes([packet[0], packet[1]]) != STUN_BINDING_SUCCESS {
        return Err("STUN response is not Binding Success".to_string());
    }
    if u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]) != STUN_MAGIC_COOKIE {
        return Err("STUN magic cookie is missing".to_string());
    }
    if packet[8..20] != transaction_id {
        return Err("STUN transaction ID does not match".to_string());
    }

    let body_length = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    if body_length % 4 != 0 {
        return Err("STUN message length is not 4-byte aligned".to_string());
    }
    let body_end = 20 + body_length;
    if packet.len() < body_end {
        return Err("STUN packet body is truncated".to_string());
    }
    if packet.len() != body_end {
        return Err("STUN packet has trailing data".to_string());
    }

    let body = &packet[20..body_end];
    let mut offset = 0;
    let mut mapped_address = None;
    while offset < body.len() {
        if body.len() - offset < 4 {
            return Err("STUN attribute header is truncated".to_string());
        }
        let attribute_type = u16::from_be_bytes([body[offset], body[offset + 1]]);
        let attribute_length =
            usize::from(u16::from_be_bytes([body[offset + 2], body[offset + 3]]));
        let value_start = offset + 4;
        let padded_length = attribute_length
            .checked_add(3)
            .ok_or_else(|| "STUN attribute length overflow".to_string())?
            & !3;
        let padded_end = value_start
            .checked_add(padded_length)
            .ok_or_else(|| "STUN attribute end overflow".to_string())?;
        if padded_end > body.len() {
            return Err("STUN attribute value is truncated".to_string());
        }

        if attribute_type == 0 {
            return Err("STUN response contains reserved attribute type 0".to_string());
        }

        if attribute_type == XOR_MAPPED_ADDRESS {
            if mapped_address.is_some() {
                return Err("STUN response has duplicate XOR-MAPPED-ADDRESS".to_string());
            }
            if attribute_length != 8 {
                return Err("IPv4 XOR-MAPPED-ADDRESS has an invalid length".to_string());
            }
            if body[value_start] != 0 {
                return Err("XOR-MAPPED-ADDRESS reserved byte is not zero".to_string());
            }
            if body[value_start + 1] != 0x01 {
                return Err("XOR-MAPPED-ADDRESS is not IPv4".to_string());
            }
            let encoded_port = u16::from_be_bytes([body[value_start + 2], body[value_start + 3]]);
            let encoded_ip = u32::from_be_bytes([
                body[value_start + 4],
                body[value_start + 5],
                body[value_start + 6],
                body[value_start + 7],
            ]);
            let port = encoded_port ^ (STUN_MAGIC_COOKIE >> 16) as u16;
            if port == 0 {
                return Err("XOR-MAPPED-ADDRESS has a zero port".to_string());
            }
            let ip = Ipv4Addr::from(encoded_ip ^ STUN_MAGIC_COOKIE);
            mapped_address = Some(SocketAddrV4::new(ip, port));
        }

        offset = padded_end;
    }

    mapped_address.ok_or_else(|| "STUN response has no IPv4 XOR-MAPPED-ADDRESS".to_string())
}
