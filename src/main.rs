use argh::FromArgs;
use bitcoin::{
    consensus::{deserialize_partial, serialize},
    p2p::message::RawNetworkMessage,
};
use bytes::BytesMut;
use eyre::Result;
use futures::future::join_all;
use std::net::SocketAddr;
use tokio::{
    io::Interest,
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    time::Instant,
};
use tracing::{debug, error, info, instrument, trace, warn};

mod handshake;

use handshake::*;

#[derive(FromArgs, PartialEq, Debug)]
/// Implementatation of the bitcoin P2P handshake.
/// This implementation is a show case of typestate pattern
struct Args {
    /// bitcoin node ip address
    #[argh(positional)]
    address: Vec<SocketAddr>,
}

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "warn,typestate-bitcoin-handshake=debug")
    }
    tracing_subscriber::fmt::init();

    let args: Args = argh::from_env();
    info!("Arguments: {args:?}");

    let now = Instant::now();
    let streams: Vec<_> = args
        .address
        .into_iter()
        .map(|a| tokio::spawn(handshake(a)))
        .collect();

    join_all(streams).await;
    info!("TOTAL Elpased -->{:#?}", now.elapsed());

    Ok(())
}

#[instrument]
async fn handshake(address: SocketAddr) -> Result<()> {
    let now = Instant::now();
    let stream = TcpStream::connect(&address).await?;
    info!("Connected at {:?}", address);

    let (writer_tx, writer_rx) = mpsc::unbounded_channel();
    let (reader_tx, reader_rx) = mpsc::unbounded_channel();

    let (reader, writer) = stream.into_split();

    let reader_jh = tokio::task::spawn(stream_reader(reader, reader_tx));
    let writer_jh = tokio::task::spawn(stream_writer(writer, writer_rx));

    let messge_received = Handshake::<Initial>::new(SendRecv::new(writer_tx, reader_rx))
        .send_version(address)?
        .receive_message()
        .await?;

    let _completed: Handshake<Completed> = match messge_received.choice() {
        Received::VerAck => {
            messge_received
                .receive_ver_state()
                .receive_version()
                .await
                .send_ack()
                .await?
        }
        Received::Version => {
            messge_received
                .send_ack_state()
                .send_ack()
                .await?
                .receive_ack()
                .await?
        }
    };

    reader_jh.abort();
    writer_jh.abort();
    info!("Elpased -->{:#?}", now.elapsed());

    Ok(())
}

#[instrument(skip_all)]
async fn stream_reader(
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
                    debug!("Bytes read {n}");
                    let mut total_consumed = 0;
                    while total_consumed < n {
                        let (msg, consumed) = deserialize_partial(&buf[total_consumed..n])?;
                        debug!(
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
                    error!("stream reader returned 4");
                    return Err(e.into());
                }
            }
        }
    }
}

#[instrument(skip_all)]
async fn stream_writer(
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
                        warn!("Only a part of the buffer is send. Data will be missing");
                        break;
                    }
                    Ok(_) => {
                        debug!("Bytes writter ok to stream");
                        break;
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        continue;
                    }
                    Err(e) => {
                        return Err(e.into());
                    }
                }
            }
        }
    }

    Ok(())
}
