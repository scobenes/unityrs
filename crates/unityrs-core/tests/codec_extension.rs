//! A registered bundle codec also supplies the writer's compressor and chunks.
use std::sync::Arc;
use unityrs_core::{
    Bytes, Error,
    budget::DecodeBuf,
    codec::{BlockCodec, Codec, Codecs, Decoded, encode},
    files::SaveError,
    limits::Meter,
};

struct Identity;
impl BlockCodec for Identity {
    fn decode(&self, input: &Bytes, _: u32, _: &mut DecodeBuf) -> Result<Decoded, Error> {
        Ok(Decoded::Input(0..input.len()))
    }
    fn compress(&self, data: &[u8], _: Codec, meter: &Arc<Meter>) -> Result<Bytes, SaveError> {
        encode::compress(data, Codec::None, meter)
    }
    fn chunk_size(&self, _: Codec, _: &Arc<Meter>) -> Result<usize, SaveError> {
        Ok(3)
    }
}

#[test]
fn a_registered_encoder_controls_chunks_and_preserves_flags() {
    let mut codecs = Codecs::unitypy();
    assert!(codecs.set(Codec::Other(5), Arc::new(Identity)).is_ok());
    let packed =
        encode::registered_chunks(b"abcdefg", 0x105, &codecs, None, &Meter::unlimited()).unwrap();
    assert_eq!(&packed.data[..], b"abcdefg");
    assert_eq!(
        packed
            .blocks
            .as_slice()
            .iter()
            .map(|b| (b.uncompressed, b.compressed, b.flags))
            .collect::<Vec<_>>(),
        [(3, 3, 0x105), (3, 3, 0x105), (1, 1, 0x105)]
    );
}

struct Captures;
impl BlockCodec for Captures {
    fn decode(&self, input: &Bytes, _: u32, _: &mut DecodeBuf) -> Result<Decoded, Error> {
        Ok(Decoded::Input(0..input.len()))
    }
    fn compressor(
        &self,
        _: Codec,
        _: &Arc<Meter>,
    ) -> Result<Option<Arc<dyn BlockCodec>>, SaveError> {
        Ok(Some(Arc::new(Identity)))
    }
    fn compress(&self, _: &[u8], which: Codec, _: &Arc<Meter>) -> Result<Bytes, SaveError> {
        Err(SaveError::UnsupportedCodec(which))
    }
    fn chunk_size(&self, which: Codec, _: &Arc<Meter>) -> Result<usize, SaveError> {
        Err(SaveError::UnsupportedCodec(which))
    }
}

#[test]
fn chunking_uses_the_operation_snapshot_for_policy_and_every_chunk() {
    let mut codecs = Codecs::unitypy();
    assert!(codecs.set(Codec::Other(5), Arc::new(Captures)).is_ok());
    let packed =
        encode::registered_chunks(b"abcdefg", 5, &codecs, None, &Meter::unlimited()).unwrap();
    assert_eq!(packed.blocks.len(), 3);
    assert_eq!(&packed.data[..], b"abcdefg");
}
