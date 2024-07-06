use bitcoin::{
    consensus::{deserialize_partial, serialize},
    p2p::message::RawNetworkMessage,
};
use bytes::BytesMut;
use eyre::Result;
use tokio_util::codec::{Decoder, Encoder};
use tracing::{debug, instrument};

pub struct BitcoinCodec;

impl Decoder for BitcoinCodec {
    type Item = RawNetworkMessage;
    type Error = eyre::Error;

    #[instrument(skip_all)]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let (msg, consumed) = match deserialize_partial(src) {
            Ok(v) => v,
            Err(e) => {
                debug!("deserialize partial failed with: {e:?}");
                return Ok(None);
            }
        };

        let _ = src.split_to(consumed);

        Ok(Some(msg))
    }
}

impl Encoder<RawNetworkMessage> for BitcoinCodec {
    type Error = eyre::Error;

    fn encode(&mut self, item: RawNetworkMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        dst.extend(serialize(&item));
        Ok(())
    }
}
