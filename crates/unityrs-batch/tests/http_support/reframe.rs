//! Align plaintext handshake messages without changing their transcript bytes.
use std::io::{self, Write};

#[derive(Default)]
pub struct Reframer {
    records: Vec<u8>,
    handshake: Vec<u8>,
    encrypted: bool,
}
impl Reframer {
    pub fn write(&mut self, output: &mut impl Write, bytes: &[u8]) -> io::Result<usize> {
        self.records.extend_from_slice(bytes);
        let mut consumed = 0;
        while self.records.len() - consumed >= 5 {
            let header = &self.records[consumed..consumed + 5];
            let length = usize::from(u16::from_be_bytes([header[3], header[4]]));
            let end = consumed + 5 + length;
            if end > self.records.len() {
                break;
            }
            let version = [header[1], header[2]];
            if !self.encrypted && header[0] == 22 {
                self.handshake
                    .extend_from_slice(&self.records[consumed + 5..end]);
                let mut sent = 0;
                while self.handshake.len() - sent >= 4 {
                    let header = &self.handshake[sent..sent + 4];
                    let length = (usize::from(header[1]) << 16)
                        | (usize::from(header[2]) << 8)
                        | usize::from(header[3]);
                    let message_end = sent + 4 + length;
                    if message_end > self.handshake.len() {
                        break;
                    }
                    // Every complete message ends its own record. The client
                    // can discard its consumed prefix before the next begins.
                    for payload in self.handshake[sent..message_end].chunks(12_283) {
                        let length = u16::try_from(payload.len()).unwrap().to_be_bytes();
                        output.write_all(&[22, version[0], version[1], length[0], length[1]])?;
                        output.write_all(payload)?;
                    }
                    sent = message_end;
                }
                self.handshake.drain(..sent);
            } else {
                if !self.handshake.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "incomplete handshake",
                    ));
                }
                // TLS 1.2 switches at CCS; TLS 1.3 encrypted records are type
                // 23, and may omit its compatibility CCS. Preserve both paths.
                self.encrypted |= matches!(header[0], 20 | 23);
                output.write_all(&self.records[consumed..end])?;
            }
            consumed = end;
        }
        self.records.drain(..consumed);
        Ok(bytes.len())
    }
    pub fn flush(&mut self, output: &mut impl Write) -> io::Result<()> {
        if !self.records.is_empty() || !self.handshake.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incomplete TLS flight",
            ));
        }
        output.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::Reframer;
    use std::io::{self, Write};

    struct ShortWriter(Vec<u8>);
    impl Write for ShortWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let count = bytes.len().min(3);
            self.0.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    fn record(kind: u8, payload: &[u8]) -> Vec<u8> {
        let length = u16::try_from(payload.len()).unwrap().to_be_bytes();
        let mut bytes = vec![kind, 3, 3, length[0], length[1]];
        bytes.extend_from_slice(payload);
        bytes
    }
    #[test]
    fn fragmented_records_and_short_writes_preserve_transcript_and_message_boundaries() {
        let first = [11, 0, 0, 3, 1, 2, 3];
        let second = [22, 0, 0, 2, 4, 5];
        let mut flight = record(22, &first[..2]);
        let mut middle = first[2..].to_vec();
        middle.extend_from_slice(&second[..4]);
        flight.extend(record(22, &middle));
        flight.extend(record(22, &second[4..]));
        let mut output = ShortWriter(Vec::new());
        let mut reframer = Reframer::default();
        for fragment in flight.chunks(2) {
            assert_eq!(
                reframer.write(&mut output, fragment).unwrap(),
                fragment.len()
            );
        }
        reframer.flush(&mut output).unwrap();
        let mut expected = record(22, &first);
        expected.extend(record(22, &second));
        assert_eq!(output.0, expected);
    }
    #[test]
    fn encrypted_records_pass_unchanged_after_ccs_or_tls13_application_data() {
        for switch in [20, 23] {
            let mut input = record(switch, &[1]);
            // Type 22 after TLS 1.2 CCS is ciphertext, even when its contents
            // happen to resemble a plaintext handshake header.
            input.extend(record(22, &[11, 0, 0, 8, 9]));
            let mut output = ShortWriter(Vec::new());
            let mut reframer = Reframer::default();
            reframer.write(&mut output, &input).unwrap();
            reframer.flush(&mut output).unwrap();
            assert_eq!(output.0, input);
        }
    }

    #[test]
    fn consecutive_large_messages_never_share_a_record_and_preserve_every_byte() {
        let mut message = vec![11, 0, 0xff, 0x78]; // 65,400 payload bytes
        message.resize(65_404, 7);
        let mut transcript = message.clone();
        transcript.extend_from_slice(&message);
        let mut wire = Vec::new();
        for payload in transcript.chunks(16_384) {
            wire.extend(record(22, payload));
        }
        let mut output = Vec::new();
        let mut reframer = Reframer::default();
        for fragment in wire.chunks(71) {
            reframer.write(&mut output, fragment).unwrap();
        }
        reframer.flush(&mut output).unwrap();
        let mut recovered = Vec::new();
        let mut cursor = 0;
        while cursor < output.len() {
            let length = usize::from(u16::from_be_bytes([output[cursor + 3], output[cursor + 4]]));
            assert!(length <= 12_283);
            assert!(length <= message.len() - recovered.len() % message.len());
            recovered.extend_from_slice(&output[cursor + 5..cursor + 5 + length]);
            cursor += 5 + length;
        }
        assert_eq!(recovered, transcript);
    }
}
