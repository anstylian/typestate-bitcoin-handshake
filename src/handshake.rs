use bitcoin::{
    p2p::{
        message::{NetworkMessage, RawNetworkMessage},
        message_network::VersionMessage,
        ServiceFlags,
    },
    Network,
};
use eyre::Result;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, instrument, warn};

/// Make the address bits in VERSION message set to zero.
const ZERO_SOCK_ADDRESS: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);

/// Handshake is the core of our design. This is a wrapper that holds our current state, and a
/// stream to communicate with the other Peers
pub struct Handshake<S> {
    stream: SendRecv,
    state: S,
}

/// The initial state.
/// Here we have the connection, and our transitions is happening when we are sending our version
/// to the remote peer. The next state is [`SendVersion`].
pub struct Initial;

/// The version is send and we are waitting for a response.
/// The response we are interested to go to the next state is [`Received::VerAck`] or [`Received::Version`].
pub struct SendVersion;

/// This describes a choice. We can not know what message will come first, the acknowledgement or
/// the version.
///
/// If [`Received::VerAck`] is received the next state will be [`ReceiveVersion`] where we will wait for the remote [`Received::Version`].
/// If [`Received::Version`] is received we go to `SentAck` to sent an acknowledgement for the version we
/// received.
pub enum Received {
    /// VerArk is received
    VerAck,
    /// Version is received
    Version,
}

/// We use [`SentAck`] and then we go to [`WaitAck`] to wait for our acknowledgement.
pub struct SentAck;

/// When we get the remote Version we come to this state to send the acknowledgement.
pub struct AckAfterVersion;

/// We wait for remote [`Received::Version`].
pub struct WaitVersion;

/// We wait for the acknowledgement message.
pub struct WaitAck;

/// When we reach `Completed` the handshake is done.
pub struct Completed;

pub struct SendRecv {
    sender: UnboundedSender<RawNetworkMessage>,
    receiver: UnboundedReceiver<RawNetworkMessage>,
}

impl SendRecv {
    pub fn new(
        sender: UnboundedSender<RawNetworkMessage>,
        receiver: UnboundedReceiver<RawNetworkMessage>,
    ) -> Self {
        Self { sender, receiver }
    }
}

impl Handshake<Initial> {
    pub fn new(stream: SendRecv) -> Self {
        Self {
            stream,
            state: Initial,
        }
    }

    // #[instrument(skip(self), fields(self.address = %self.address))]
    #[instrument(skip(self))]
    pub fn sent_version(self, address: SocketAddr) -> Result<Handshake<SendVersion>> {
        debug!("Sending Version");
        let version_message = get_version_message(&address)?;
        let packet = RawNetworkMessage::new(
            Network::Bitcoin.magic(),
            NetworkMessage::Version(version_message),
        );

        self.stream.sender.send(packet)?;

        Ok(Handshake {
            stream: self.stream,
            state: SendVersion,
        })
    }
}

impl Handshake<SendVersion> {
    #[instrument(skip_all)]
    pub async fn receive_message(mut self) -> Result<Handshake<Received>> {
        debug!("Wait for message");
        while let Some(msg) = self.stream.receiver.recv().await {
            debug!("Receive {msg:?}");
            let msg = read_message(msg);
            match msg {
                Some(m) => {
                    return Ok(Handshake {
                        stream: self.stream,
                        state: m,
                    })
                }
                None => { /* ignore */ }
            }
        }

        eyre::bail!("Unexcpected sequence")
    }
}

impl Handshake<Received> {
    /// If Version is received first, use this to go at the next state to send `VerAck`.
    pub fn send_ack_state(self) -> Handshake<SentAck> {
        Handshake {
            stream: self.stream,
            state: SentAck,
        }
    }

    /// If `VerAck` is received first, use this to go at the next state to wait for the remote
    /// `Version`.
    pub fn receive_ver_state(self) -> Handshake<WaitVersion> {
        Handshake {
            stream: self.stream,
            state: WaitVersion,
        }
    }

    pub fn choice(&self) -> &Received {
        &self.state
    }
}

impl Handshake<SentAck> {
    /// Send `VerAck` and go to the next state to wait for `VerAck`.
    #[instrument(skip_all)]
    pub async fn send_ack(self) -> Result<Handshake<WaitAck>> {
        debug!("Sent VerAck");
        let packet = RawNetworkMessage::new(Network::Bitcoin.magic(), NetworkMessage::Verack);
        self.stream.sender.send(packet)?;
        return Ok(Handshake {
            stream: self.stream,
            state: WaitAck,
        });
    }
}

impl Handshake<WaitAck> {
    /// Get `VerArc` and complete.
    #[instrument(skip_all)]
    pub async fn receive_ack(mut self) -> Result<Handshake<Completed>> {
        while let Some(msg) = self.stream.receiver.recv().await {
            let msg = read_message(msg);
            match msg {
                Some(Received::VerAck) => {
                    debug!("Received VerAck");
                    break;
                }
                Some(Received::Version) => {
                    warn!("Something is wrong. Receive Version twice");
                }
                None => { /* ignore */ }
            }
        }
        return Ok(Handshake {
            stream: self.stream,
            state: Completed,
        });
    }
}

impl Handshake<WaitVersion> {
    /// Wait for remote Version
    #[instrument(skip_all)]
    pub async fn receive_version(mut self) -> Handshake<AckAfterVersion> {
        while let Some(msg) = self.stream.receiver.recv().await {
            let msg = read_message(msg);
            match msg {
                Some(Received::VerAck) => {
                    warn!("Something is wrong. Receive VerAck twice");
                }
                Some(Received::Version) => {
                    debug!("Received Version");
                    break;
                }
                None => { /* ignore */ }
            }
        }

        Handshake {
            stream: self.stream,
            state: AckAfterVersion,
        }
    }
}
impl Handshake<AckAfterVersion> {
    /// Sent `VerAck` and complete
    #[instrument(skip_all)]
    pub async fn send_ack(self) -> Result<Handshake<Completed>> {
        debug!("Sent VerAck");
        let packet = RawNetworkMessage::new(Network::Bitcoin.magic(), NetworkMessage::Verack);
        self.stream.sender.send(packet)?;
        return Ok(Handshake {
            stream: self.stream,
            state: Completed,
        });
    }
}

/// Translate received message into our types
#[instrument(skip_all)]
fn read_message(package: RawNetworkMessage) -> Option<Received> {
    let msg_type = package.cmd().to_string();
    match package.payload() {
        NetworkMessage::Verack => {
            debug!("Receive Verack");
            Some(Received::VerAck)
        }
        NetworkMessage::Version(v) => {
            debug!("Receive Version: {:?}", v);
            Some(Received::Version)
        }
        _ => {
            warn!(
                "Received message type not part of handshake: {:?}",
                msg_type
            );
            None
        }
    }
}

#[cfg(test)]
pub fn static_version_message_package() -> RawNetworkMessage {
    let unix_epoch = 0;

    let remote_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
    let remote_address = bitcoin::p2p::Address::new(&remote_address, ServiceFlags::NONE);
    let zero_bitcoin_address = bitcoin::p2p::Address::new(&ZERO_SOCK_ADDRESS, ServiceFlags::NONE);

    let version_message = version_message(remote_address, zero_bitcoin_address, unix_epoch);

    RawNetworkMessage::new(
        Network::Bitcoin.magic(),
        NetworkMessage::Version(version_message),
    )
}

fn get_version_message(remote_address: &SocketAddr) -> Result<VersionMessage> {
    let unix_epoch: i64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .try_into()?;

    let remote_address: bitcoin::p2p::Address =
        bitcoin::p2p::Address::new(remote_address, ServiceFlags::NONE);

    let zero_bitcoin_address = bitcoin::p2p::Address::new(&ZERO_SOCK_ADDRESS, ServiceFlags::NONE);

    Ok(version_message(
        remote_address,
        zero_bitcoin_address,
        unix_epoch,
    ))
}

/// Construct the Version Message
///
/// Parts
/// The first four are the message header
/// 1. Magic bytes: static sequence of bytes, to indicate the start of the message (0xf9 0xbe 0xb4
///    0xd9), 4 bytes
/// 2. Command: "version" as ASCII bytes, 12 bytes
/// 3. Size: as little-endian, 4 bytes
/// 4. Checksum: 4 bytes
///
/// Version message payload
///
/// Protocol Version: little-endian, 4 bytes
/// Services: 8 bytes, bit-filed, little-endian, indicating the supported services of the node. (we
///     can use 0 for testing)
/// Time: our local Unix timestamps
/// Remote Services: 8 bytes, use 0 for the moment
/// Remote IP: IPv6, big-endian 16-bytes
/// Remote Port: 2 bytes, big-endian
/// Local Services:
/// Local IP:
/// Nonce:
/// User Agent:
/// Last Block:
fn version_message(
    remote_address: bitcoin::p2p::Address,
    local_address: bitcoin::p2p::Address,
    unix_epoch: i64,
) -> VersionMessage {
    VersionMessage::new(
        ServiceFlags::NONE,
        unix_epoch,
        remote_address,
        local_address,
        0,
        "TEST NODE".to_string(),
        0,
    )
}
