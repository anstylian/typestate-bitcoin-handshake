//! Typestate Bitcoin Handshake
//!
//!
//! # Bitcoin handshake implemented using typestate pattern
//!
//! ## Why Typestate?
//! Typestate is a design pattern that allows as to leverage the type system
//! and describe our flow. The benefit is that at compile time we will know
//! that our flow is valid
//!
//! ## Bitcoin handshake
//! The handshake of bitcoin is simple.
//!
//! ```text
//! ┌─────┐               ┌──────┐
//! │Local│               │Remote│
//! └──┬──┘               └───┬──┘
//!    │       Version        │
//!    │─────────────────────►│
//!    │       VerAck         │
//!    │◄─────────────────────│
//!    │       Version        │
//!    │◄─────────────────────│
//!    │       VerAck         │
//!    │─────────────────────►│
//!    │                      │
//! ┌──┴──┐               ┌───┴──┐
//! │Local│               │Remote│
//! └─────┘               └──────┘
//! ```
//!
//! The protocol has no guarantees for the messages order.
//! The `Version` message of the local host is the first message that starts the handshake, but we can not
//! know if the `VerAck` or the `Version` will come first. We just need to make sure that each `Version` message
//! will be followed by an `VerAck` message.
//!
//! ## Typestate
//! ```text
//! Init
//!   -> Sent Version
//!   -> choice Receive VerAck Or Version {
//!     VerAck
//!       -> Wait for Version
//!       -> Sent VerAck
//!     Version
//!       -> Sent VerAck
//!       -> Wait for VerAck
//!   }
//!   -> Complete
//! ```
//!
use argh::FromArgs;
use eyre::{eyre, Result};
use futures::future::join_all;
use std::net::SocketAddr;
use tokio::{net::TcpStream, time::Instant};
use tokio_util::codec::Framed;
use tracing::{error, info, instrument};

mod codec;
mod handshake;

use crate::{codec::BitcoinCodec, handshake::*};

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
        std::env::set_var("RUST_LOG", "warn,typestate_bitcoin_handshake=info")
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

    let ret = join_all(streams).await;
    info!("Total elpased time {:#?}", now.elapsed());

    for join_res in ret {
        let Ok(Ok(_)) = join_res else {
            error!("{join_res:?}");
            continue;
        };
    }

    Ok(())
}

/// Handling bitcoin handshake with the target address.
#[instrument]
async fn handshake(address: SocketAddr) -> Result<()> {
    let now = Instant::now();
    let stream = TcpStream::connect(&address)
        .await
        .map_err(|e| eyre!("remote: {address:?}, error: {e:?}"))?;
    info!("Connected at {:?}", address);

    let transport = Framed::new(stream, BitcoinCodec {});

    let res = typestate(address, transport)
        .await
        .map_err(|e| eyre!("remote: {address:?}, error: {e:?}"))?;

    res.close_stream().await?;

    info!("Handshake completed in {:#?}", now.elapsed());

    Ok(())
}

async fn typestate(
    address: SocketAddr,
    transport: Framed<TcpStream, BitcoinCodec>,
) -> Result<Handshake<Completed>> {
    let messge_received = Handshake::<Initial>::new(transport)
        .sent_version(address)
        .await?
        .receive_message()
        .await?;

    let res = match messge_received.choice() {
        Received::VerAck => {
            messge_received
                .receive_ver_state()
                .receive_version()
                .await?
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
    use futures::SinkExt;
    use std::net::{IpAddr, Ipv4Addr};
    use tokio::{net::TcpListener, task::JoinHandle};
    use tokio_stream::StreamExt;

    use super::*;

    async fn test_setup(port: u16) -> (Framed<TcpStream, BitcoinCodec>, JoinHandle<()>) {
        // Start server
        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port);
        let server = TcpListener::bind(&address)
            .await
            .expect("Failed to pinf address");

        // Start client
        let jh = tokio::task::spawn(async move {
            handshake(address).await.expect("Handshake failed");
        });

        // Accept client
        let (stream, _) = server.accept().await.expect("Failed to accept client");
        let transport = Framed::new(stream, BitcoinCodec {});
        (transport, jh)
    }

    #[tokio::test]
    async fn message_sequence_1() {
        let (mut transport, jh) = test_setup(8080).await;

        let Some(Ok(msg)) = transport.next().await else {
            panic!("Failed to get client version");
        };
        let msg = read_message(msg).expect("Failed to read message");
        assert_eq!(msg, Received::Version);

        // Send VerAck
        let ack_packet = RawNetworkMessage::new(Network::Bitcoin.magic(), NetworkMessage::Verack);
        transport
            .send(ack_packet.clone())
            .await
            .expect("Failed to send ACK message");

        // Send Version
        let remote_version = static_version_message_package();
        transport
            .send(remote_version)
            .await
            .expect("Failed to send message");

        // Recv VerAck
        let Ok(ack) = transport.next().await.expect("Failed to get ACK message") else {
            panic!("Faild to get ack");
        };
        assert_eq!(ack, ack_packet);

        jh.await.expect("task failed");
    }

    #[tokio::test]
    async fn message_sequence_2() {
        let (mut transport, jh) = test_setup(8081).await;

        let Some(Ok(msg)) = transport.next().await else {
            panic!("Failed to get client version");
        };
        let msg = read_message(msg).expect("Failed to read message");
        assert_eq!(msg, Received::Version);

        // Send Version
        let remote_version = static_version_message_package();
        transport
            .send(remote_version)
            .await
            .expect("Failed to send message");

        // Send VerAck
        let ack_packet = RawNetworkMessage::new(Network::Bitcoin.magic(), NetworkMessage::Verack);
        transport
            .send(ack_packet.clone())
            .await
            .expect("Failed to send ACK message");

        let Some(Ok(ack)) = transport.next().await else {
            panic!("Failed to Ack message");
        };
        assert_eq!(ack, ack_packet);

        jh.await.expect("task failed");
    }

    #[tokio::test]
    async fn message_sequence_3() {
        let (mut transport, jh) = test_setup(8082).await;

        let Some(Ok(msg)) = transport.next().await else {
            panic!("Failed to get client version");
        };
        let msg = read_message(msg).expect("Failed to read message");
        assert_eq!(msg, Received::Version);

        // Send Version
        let remote_version = static_version_message_package();
        transport
            .send(remote_version)
            .await
            .expect("Failed to send message");

        // Recv VerAck
        let Some(Ok(ack)) = transport.next().await else {
            panic!("Failed to Ack message");
        };

        // Send VerAck
        let ack_packet = RawNetworkMessage::new(Network::Bitcoin.magic(), NetworkMessage::Verack);
        assert_eq!(ack, ack_packet);
        transport
            .send(ack_packet)
            .await
            .expect("Failed to send ACK message");

        jh.await.expect("task failed");
    }
}
