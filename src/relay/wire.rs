// Copyright 2025 John A Keeney - Entrouter
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

/// Wire protocol for tunnel packets.
///
/// Frame layout (all fields little-endian):
/// ```text
/// [1 byte]  packet_type
/// [8 bytes] sequence number (u64)
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

pub const HEADER_SIZE: usize = 11; // type(1) + seq(8) + len(2)
pub const AUTH_TAG_SIZE: usize = 16;
pub const MAX_PAYLOAD: usize = 1400; // safe for MTU 1500
pub const MAX_PACKET: usize = HEADER_SIZE + MAX_PAYLOAD + AUTH_TAG_SIZE;

/// Encode a frame header into a buffer. Returns bytes written (always 11).
pub fn encode_header(buf: &mut [u8], packet_type: u8, seq: u64, payload_len: u16) {
    buf[0] = packet_type;
    buf[1..9].copy_from_slice(&seq.to_le_bytes());
    buf[9..11].copy_from_slice(&payload_len.to_le_bytes());
}

/// Decode a frame header from a buffer.
pub fn decode_header(buf: &[u8]) -> (u8, u64, u16) {
    let packet_type = buf[0];
    let seq = u64::from_le_bytes([
        buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7], buf[8],
    ]);
    let payload_len = u16::from_le_bytes([buf[9], buf[10]]);
    (packet_type, seq, payload_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_header() {
        let mut buf = [0u8; HEADER_SIZE];
        encode_header(&mut buf, PACKET_DATA, 1234u64, 500);
        let (ptype, seq, len) = decode_header(&buf);
        assert_eq!(ptype, PACKET_DATA);
        assert_eq!(seq, 1234u64);
        assert_eq!(len, 500);
    }

    #[test]
    fn all_packet_types() {
        for ptype in [
            PACKET_DATA,
            PACKET_PARITY,
            PACKET_PING,
            PACKET_PONG,
            PACKET_CONTROL,
        ] {
            let mut buf = [0u8; HEADER_SIZE];
            encode_header(&mut buf, ptype, 0, 0);
            let (decoded_type, _, _) = decode_header(&buf);
            assert_eq!(decoded_type, ptype);
        }
    }

    #[test]
    fn max_seq_and_len() {
        let mut buf = [0u8; HEADER_SIZE];
        encode_header(&mut buf, PACKET_DATA, u64::MAX, u16::MAX);
        let (ptype, seq, len) = decode_header(&buf);
        assert_eq!(ptype, PACKET_DATA);
        assert_eq!(seq, u64::MAX);
        assert_eq!(len, u16::MAX);
    }

    #[test]
    fn zero_values() {
        let mut buf = [0u8; HEADER_SIZE];
        encode_header(&mut buf, 0, 0, 0);
        let (ptype, seq, len) = decode_header(&buf);
        assert_eq!(ptype, 0);
        assert_eq!(seq, 0);
        assert_eq!(len, 0);
    }

    #[test]
    fn payload_within_mtu() {
        // Verify MAX_PAYLOAD + HEADER_SIZE + AUTH_TAG_SIZE fits in a reasonable MTU
        assert!(MAX_PACKET <= 1500);
        assert_eq!(MAX_PACKET, HEADER_SIZE + MAX_PAYLOAD + AUTH_TAG_SIZE);
    }
}
