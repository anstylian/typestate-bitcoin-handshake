use bitcoin::{
    consensus::{deserialize_partial, serialize},
    p2p::message::RawNetworkMessage,
};
use bytes::BytesMut;
use eyre::Result;
use tokio::{
    io::Interest,
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};
use tracing::{error, instrument, trace, warn};

/// Handle incoming messages from the connection.
#[instrument(skip_all)]
pub async fn stream_reader(
    reader: OwnedReadHalf,
    reader_tx: UnboundedSender<RawNetworkMessage>,
) -> Result<()> {
    let mut buf = BytesMut::with_capacity(1024 * 8);
    loop {
        let ready = reader.ready(Interest::READABLE).await?;
        if ready.is_readable() {
            // Try to read data, this may still fail with `WouldBlock`
            // if the readiness event is a false positive.
            match reader.try_read_buf(&mut buf) {
                Ok(0) => {
                    // error!("TCP stream closed");
                }
                Ok(n) => {
                    trace!("Bytes read {n}");
                    let mut total_consumed = 0;
                    while total_consumed < n {
                        let (msg, consumed) = deserialize_partial(&buf[total_consumed..n])?;
                        trace!(
                            "Deserilization consumed {} out of {n} bytes",
                            consumed + total_consumed
                        );
                        trace!("Recv: {msg:?}");
                        reader_tx.send(msg)?;
                        total_consumed += consumed;
                    }

                    // buf.unsplit(bytes_read);
                    buf.clear();
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    continue;
                }
                Err(e) => {
                    error!("Read failed with: {e:?}");
                    return Err(e.into());
                }
            }
        }
    }
}

/// Handle outcoming messages to the connection.
#[instrument(skip_all)]
pub async fn stream_writer(
    writer: OwnedWriteHalf,
    writer_rx: UnboundedReceiver<RawNetworkMessage>,
) -> Result<()> {
    let mut writer_rx = writer_rx;
    while let Some(msg) = writer_rx.recv().await {
        let buf = serialize(&msg);
        let ready = writer.ready(Interest::WRITABLE).await?;
        if ready.is_writable() {
            loop {
                match writer.try_write(&buf) {
                    Ok(n) if n < buf.len() => {
                        // TODO: this should be handle
                        warn!("Only a part of the buffer is send. Data will be missing");
                        break;
                    }
                    Ok(_) => {
                        trace!("Bytes writter ok to stream");
                        break;
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        continue;
                    }
                    Err(e) => {
                        error!("Write failed with: {e:?}");
                        return Err(e.into());
                    }
                }
            }
        }
    }

    Ok(())
}
