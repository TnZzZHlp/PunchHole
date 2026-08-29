use std::net::{Ipv4Addr, SocketAddrV4};

use PunchHole::{
    STUN_BINDING_SUCCESS, STUN_MAGIC_COOKIE, XOR_MAPPED_ADDRESS, parse_xor_mapped_address,
};

#[test]
fn parses_ipv4_xor_mapped_address() {
    let transaction_id = [0x11; 12];
    let public_ip = Ipv4Addr::new(203, 0, 113, 7);
    let public_port = 42_424;
    let encoded_port = public_port ^ (STUN_MAGIC_COOKIE >> 16) as u16;
    let encoded_ip = u32::from_be_bytes(public_ip.octets()) ^ STUN_MAGIC_COOKIE;
    let mut packet = vec![0; 20 + 12];
    packet[0..2].copy_from_slice(&STUN_BINDING_SUCCESS.to_be_bytes());
    packet[2..4].copy_from_slice(&12_u16.to_be_bytes());
    packet[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    packet[8..20].copy_from_slice(&transaction_id);
    packet[20..22].copy_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
    packet[22..24].copy_from_slice(&8_u16.to_be_bytes());
    packet[24] = 0;
    packet[25] = 1;
    packet[26..28].copy_from_slice(&encoded_port.to_be_bytes());
    packet[28..32].copy_from_slice(&encoded_ip.to_be_bytes());

    assert_eq!(
        parse_xor_mapped_address(&packet, transaction_id).unwrap(),
        SocketAddrV4::new(public_ip, public_port)
    );
}

#[test]
fn rejects_stun_wrong_type_or_transaction() {
    let transaction_id = [0x22; 12];
    let mut packet = vec![0; 20 + 12];
    packet[0..2].copy_from_slice(&STUN_BINDING_SUCCESS.to_be_bytes());
    packet[2..4].copy_from_slice(&12_u16.to_be_bytes());
    packet[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    packet[8..20].copy_from_slice(&transaction_id);
    packet[20..22].copy_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
    packet[22..24].copy_from_slice(&8_u16.to_be_bytes());
    packet[25] = 1;
    packet[26..28].copy_from_slice(&(0xa5b8_u16 ^ (STUN_MAGIC_COOKIE >> 16) as u16).to_be_bytes());
    packet[28..32]
        .copy_from_slice(&(u32::from_be_bytes([203, 0, 113, 7]) ^ STUN_MAGIC_COOKIE).to_be_bytes());

    let mut wrong_type = packet.clone();
    wrong_type[0..2].copy_from_slice(&0x0111_u16.to_be_bytes());
    assert!(parse_xor_mapped_address(&wrong_type, transaction_id).is_err());

    let mut wrong_transaction = packet;
    wrong_transaction[19] ^= 1;
    assert!(parse_xor_mapped_address(&wrong_transaction, transaction_id).is_err());
}

#[test]
fn rejects_stun_malformed_framing() {
    let transaction_id = [0x33; 12];
    let mut packet = vec![0; 20 + 12];
    packet[0..2].copy_from_slice(&STUN_BINDING_SUCCESS.to_be_bytes());
    packet[2..4].copy_from_slice(&12_u16.to_be_bytes());
    packet[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    packet[8..20].copy_from_slice(&transaction_id);
    packet[20..22].copy_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
    packet[22..24].copy_from_slice(&8_u16.to_be_bytes());
    packet[25] = 1;
    packet[26..28].copy_from_slice(&(0xa5b8_u16 ^ (STUN_MAGIC_COOKIE >> 16) as u16).to_be_bytes());
    packet[28..32]
        .copy_from_slice(&(u32::from_be_bytes([203, 0, 113, 7]) ^ STUN_MAGIC_COOKIE).to_be_bytes());

    let mut unaligned = packet.clone();
    unaligned[2..4].copy_from_slice(&11_u16.to_be_bytes());
    assert!(parse_xor_mapped_address(&unaligned, transaction_id).is_err());

    let mut truncated = packet;
    truncated.truncate(31);
    assert!(parse_xor_mapped_address(&truncated, transaction_id).is_err());
}

#[test]
fn rejects_attribute_truncated_after_xor_mapped_address() {
    let transaction_id = [0x44; 12];
    let public_ip = Ipv4Addr::new(203, 0, 113, 7);
    let public_port = 42_424;
    let encoded_port = public_port ^ (STUN_MAGIC_COOKIE >> 16) as u16;
    let encoded_ip = u32::from_be_bytes(public_ip.octets()) ^ STUN_MAGIC_COOKIE;
    let mut packet = vec![0; 20 + 16];
    packet[0..2].copy_from_slice(&STUN_BINDING_SUCCESS.to_be_bytes());
    packet[2..4].copy_from_slice(&16_u16.to_be_bytes());
    packet[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    packet[8..20].copy_from_slice(&transaction_id);
    packet[20..22].copy_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
    packet[22..24].copy_from_slice(&8_u16.to_be_bytes());
    packet[25] = 1;
    packet[26..28].copy_from_slice(&encoded_port.to_be_bytes());
    packet[28..32].copy_from_slice(&encoded_ip.to_be_bytes());
    packet[32..34].copy_from_slice(&0x0006_u16.to_be_bytes());
    packet[34..36].copy_from_slice(&4_u16.to_be_bytes());

    assert!(parse_xor_mapped_address(&packet, transaction_id).is_err());
}

#[test]
fn rejects_nonzero_xor_mapped_address_reserved_byte() {
    let transaction_id = [0x55; 12];
    let public_ip = Ipv4Addr::new(203, 0, 113, 7);
    let public_port = 42_424;
    let encoded_port = public_port ^ (STUN_MAGIC_COOKIE >> 16) as u16;
    let encoded_ip = u32::from_be_bytes(public_ip.octets()) ^ STUN_MAGIC_COOKIE;
    let mut packet = vec![0; 20 + 12];
    packet[0..2].copy_from_slice(&STUN_BINDING_SUCCESS.to_be_bytes());
    packet[2..4].copy_from_slice(&12_u16.to_be_bytes());
    packet[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    packet[8..20].copy_from_slice(&transaction_id);
    packet[20..22].copy_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
    packet[22..24].copy_from_slice(&8_u16.to_be_bytes());
    packet[24] = 1;
    packet[25] = 1;
    packet[26..28].copy_from_slice(&encoded_port.to_be_bytes());
    packet[28..32].copy_from_slice(&encoded_ip.to_be_bytes());

    assert!(parse_xor_mapped_address(&packet, transaction_id).is_err());
}

#[test]
fn ignores_nonzero_optional_attribute_padding() {
    let transaction_id = [0x66; 12];
    let public_ip = Ipv4Addr::new(203, 0, 113, 7);
    let public_port = 42_424;
    let encoded_port = public_port ^ (STUN_MAGIC_COOKIE >> 16) as u16;
    let encoded_ip = u32::from_be_bytes(public_ip.octets()) ^ STUN_MAGIC_COOKIE;
    let mut packet = vec![0; 20 + 20];
    packet[0..2].copy_from_slice(&STUN_BINDING_SUCCESS.to_be_bytes());
    packet[2..4].copy_from_slice(&20_u16.to_be_bytes());
    packet[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    packet[8..20].copy_from_slice(&transaction_id);
    packet[20..22].copy_from_slice(&0x8001_u16.to_be_bytes());
    packet[22..24].copy_from_slice(&1_u16.to_be_bytes());
    packet[24] = 0x42;
    packet[25..28].copy_from_slice(&[0xaa, 0xbb, 0xcc]);
    packet[28..30].copy_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
    packet[30..32].copy_from_slice(&8_u16.to_be_bytes());
    packet[33] = 1;
    packet[34..36].copy_from_slice(&encoded_port.to_be_bytes());
    packet[36..40].copy_from_slice(&encoded_ip.to_be_bytes());

    assert_eq!(
        parse_xor_mapped_address(&packet, transaction_id).unwrap(),
        SocketAddrV4::new(public_ip, public_port)
    );
}

#[test]
fn rejects_reserved_stun_attribute_type_zero() {
    let transaction_id = [0x66; 12];
    let public_ip = Ipv4Addr::new(203, 0, 113, 7);
    let public_port = 42_424;
    let encoded_port = public_port ^ (STUN_MAGIC_COOKIE >> 16) as u16;
    let encoded_ip = u32::from_be_bytes(public_ip.octets()) ^ STUN_MAGIC_COOKIE;
    let mut packet = vec![0; 20 + 16];
    packet[0..2].copy_from_slice(&STUN_BINDING_SUCCESS.to_be_bytes());
    packet[2..4].copy_from_slice(&16_u16.to_be_bytes());
    packet[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    packet[8..20].copy_from_slice(&transaction_id);
    packet[20..22].copy_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
    packet[22..24].copy_from_slice(&8_u16.to_be_bytes());
    packet[25] = 1;
    packet[26..28].copy_from_slice(&encoded_port.to_be_bytes());
    packet[28..32].copy_from_slice(&encoded_ip.to_be_bytes());
    packet[32..34].copy_from_slice(&0_u16.to_be_bytes());
    packet[34..36].copy_from_slice(&0_u16.to_be_bytes());

    assert!(parse_xor_mapped_address(&packet, transaction_id).is_err());
}
