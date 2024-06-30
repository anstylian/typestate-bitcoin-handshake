use argh::FromArgs;
use eyre::Result;
use futures::future::join_all;
use std::net::SocketAddr;
use tokio::{net::TcpStream, sync::mpsc, time::Instant};
use tracing::{info, instrument};

mod connection_handler;
mod handshake;

use crate::{
    connection_handler::{stream_reader, stream_writer},
    handshake::*,
};

#[derive(FromArgs, PartialEq, Debug)]
/// Implementation of the bitcoin P2P handshake.
/// This implementation is a show case of typestate pattern
struct Args {
    /// bitcoin node IP address
    #[argh(positional)]
    address: Vec<SocketAddr>,
}

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "warn,typestate=debug")
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
    info!("TOTAL Elpased time -->{:#?}", now.elapsed());

    Ok(())
}

/// Handling bitcoin handshake with the target address.
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
        .sent_version(address)?
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
