use argh::FromArgs;
use bitcoin::p2p::message::RawNetworkMessage;
use eyre::Result;
use futures::future::join_all;
use std::net::SocketAddr;
use tokio::{
    net::TcpStream,
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    time::Instant,
};
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

    typestate(address, writer_tx, reader_rx).await?;

    reader_jh.abort();
    writer_jh.abort();
    info!("Elpased -->{:#?}", now.elapsed());

    Ok(())
}

async fn typestate(
    address: SocketAddr,
    writer_tx: UnboundedSender<RawNetworkMessage>,
    reader_rx: UnboundedReceiver<RawNetworkMessage>,
) -> Result<Handshake<Completed>> {
    let messge_received = Handshake::<Initial>::new(SendRecv::new(writer_tx, reader_rx))
        .sent_version(address)?
        .receive_message()
        .await?;

    let res = match messge_received.choice() {
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

    Ok(res)
}

#[cfg(test)]
mod handshake_test {
    use bitcoin::{
        p2p::message::{NetworkMessage, RawNetworkMessage},
        Network,
    };
    use std::net::{IpAddr, Ipv4Addr};
    use tokio::{
        sync::mpsc::{UnboundedReceiver, UnboundedSender},
        task::JoinHandle,
    };

    use super::*;

    struct TestSetup {
        writer_rx: UnboundedReceiver<RawNetworkMessage>,
        reader_tx: UnboundedSender<RawNetworkMessage>,
        jh: JoinHandle<()>,
    }

    fn test_setup() -> TestSetup {
        let (writer_tx, writer_rx) = mpsc::unbounded_channel();
        let (reader_tx, reader_rx) = mpsc::unbounded_channel();

        let jh: JoinHandle<()> = tokio::task::spawn(async {
            let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
            typestate(address, writer_tx, reader_rx)
                .await
                .expect("Typestate Failed");
        });

        TestSetup {
            writer_rx,
            reader_tx,
            jh,
        }
    }

    #[tokio::test]
    async fn handshake_verack_then_version() {
        let TestSetup {
            mut writer_rx,
            reader_tx,
            jh,
        } = test_setup();

        writer_rx.recv().await.expect("Failed to get Version");

        // Send VerAck
        let ack_packet = RawNetworkMessage::new(Network::Bitcoin.magic(), NetworkMessage::Verack);
        reader_tx
            .send(ack_packet)
            .expect("Failed to send ACK message");

        // Send Version
        let remote_version = static_version_message_package();
        reader_tx
            .send(remote_version)
            .expect("Failed to send message");

        // Recv VerAck
        writer_rx.recv().await.expect("Failed to get ACK message");

        jh.await.expect("task failed");
    }

    #[tokio::test]
    async fn handshake_version_then_verack() {
        let TestSetup {
            mut writer_rx,
            reader_tx,
            jh,
        } = test_setup();

        writer_rx.recv().await.expect("Failed to get Version");

        // Send Version
        let remote_version = static_version_message_package();
        reader_tx
            .send(remote_version)
            .expect("Failed to send message");

        // Send VerAck
        let ack_packet = RawNetworkMessage::new(Network::Bitcoin.magic(), NetworkMessage::Verack);
        reader_tx
            .send(ack_packet)
            .expect("Failed to send ACK message");

        // Recv VerAck
        writer_rx.recv().await.expect("Failed to get ACK message");

        jh.await.expect("task failed");
    }

    #[tokio::test]
    async fn handshake_version_then_verack2() {
        let TestSetup {
            mut writer_rx,
            reader_tx,
            jh,
        } = test_setup();

        writer_rx.recv().await.expect("Failed to get Version");

        // Send Version
        let remote_version = static_version_message_package();
        reader_tx
            .send(remote_version)
            .expect("Failed to send message");

        // Recv VerAck
        writer_rx.recv().await.expect("Failed to get ACK message");

        // Send VerAck
        let ack_packet = RawNetworkMessage::new(Network::Bitcoin.magic(), NetworkMessage::Verack);
        reader_tx
            .send(ack_packet)
            .expect("Failed to send ACK message");

        jh.await.expect("task failed");
    }
}
