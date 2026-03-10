/// Wire protocol for tunnel packets.
///
/// Frame layout (all fields little-endian):
/// ```text
/// [1 byte]  packet_type
/// [2 bytes] sequence number
/// [2 bytes] payload length
/// [N bytes] payload (encrypted)
/// [16 bytes] auth tag (Poly1305)
/// ```
///
/// Packet types:
/// - 0x01: Data shard (FEC original data)
/// - 0x02: Parity shard (FEC recovery data)
/// - 0x03: Probe ping
/// - 0x04: Probe pong
/// - 0x05: Control (mesh routing updates)
pub const PACKET_DATA: u8 = 0x01;
pub const PACKET_PARITY: u8 = 0x02;
pub const PACKET_PING: u8 = 0x03;
pub const PACKET_PONG: u8 = 0x04;
pub const PACKET_CONTROL: u8 = 0x05;

pub const HEADER_SIZE: usize = 5; // type(1) + seq(2) + len(2)
pub const AUTH_TAG_SIZE: usize = 16;
pub const MAX_PAYLOAD: usize = 1400; // safe for MTU 1500
pub const MAX_PACKET: usize = HEADER_SIZE + MAX_PAYLOAD + AUTH_TAG_SIZE;

/// Encode a frame header into a buffer. Returns bytes written (always 5).
pub fn encode_header(buf: &mut [u8], packet_type: u8, seq: u16, payload_len: u16) {
    buf[0] = packet_type;
    buf[1..3].copy_from_slice(&seq.to_le_bytes());
    buf[3..5].copy_from_slice(&payload_len.to_le_bytes());
}

/// Decode a frame header from a buffer.
pub fn decode_header(buf: &[u8]) -> (u8, u16, u16) {
    let packet_type = buf[0];
    let seq = u16::from_le_bytes([buf[1], buf[2]]);
    let payload_len = u16::from_le_bytes([buf[3], buf[4]]);
    (packet_type, seq, payload_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_header() {
        let mut buf = [0u8; HEADER_SIZE];
        encode_header(&mut buf, PACKET_DATA, 1234, 500);
        let (ptype, seq, len) = decode_header(&buf);
        assert_eq!(ptype, PACKET_DATA);
        assert_eq!(seq, 1234);
        assert_eq!(len, 500);
    }
}
