use std::{
    fmt,
    net::{Ipv4Addr, SocketAddrV4, IpAddr, SocketAddr},
};

use serde::{de::Visitor, Deserialize, Deserializer, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Event {
    /// The download has just started.
    Started,
    /// The download has completed.
    Completed,
    /// The download has been stopped, not the same as Completed.
    Stopped,
    /// This is the same as not sending the `event` query parameter.
    Empty,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Tracker {
    /// A unique id for this client.
    peer_id: String,
    /// Optional ip address of the peer.
    ip: Option<Ipv4Addr>,
    /// The port number this client listens on.
    port: u16,
    /// The total amount uploaded.
    uploaded: usize,
    /// The total amount downloaded.
    downloaded: usize,
    /// The number of bytes left to download.
    left: usize,
    /// Should the peer list use the compact representation.
    compact: usize,
    // Optional key that announces the state of the client
    event: Option<Event>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Peer {
    ip: SocketAddr,
    peer_id: String,
}

#[derive(Debug)]
pub(crate) struct Peers(pub(crate) Vec<SocketAddrV4>);

#[derive(Debug)]
pub(crate) enum PeersKind {
    Objects(Vec<Peer>),
    Bytes(Peers),
}

#[derive(Debug, Deserialize)]
pub(crate) struct TrackerResponse {
    pub(crate) interval: usize,
    #[serde(deserialize_with = "peer_deser")]
    pub(crate) peers: PeersKind,
}

impl Tracker {
    pub(crate) fn new(total_len: usize) -> Self {
        Self {
            peer_id: "00112233445566778899".to_string(),
            ip: None,
            port: 6881,
            uploaded: 0,
            downloaded: 0,
            left: total_len,
            compact: 1,
            event: None,
        }
    }
}

impl PeersKind {
    pub(crate) fn socket_addr(&self) -> Vec<SocketAddr> {
        if let PeersKind::Bytes(peer) = self {
            peer.0.iter().map(|p| SocketAddr::V4(*p)).collect()
        } else if let PeersKind::Objects(peer) = self {
            peer.iter().map(|p| p.ip).collect()
        } else {
            panic!("enums are broken in rust somehow")
        }
    }
}

fn peer_deser<'de, D>(deserializer: D) -> Result<PeersKind, D::Error>
where
    D: Deserializer<'de>,
{
    struct PeersVisitor;

    impl<'de> Visitor<'de> for PeersVisitor {
        type Value = PeersKind;
        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("chunks of 6 bytes containing an IP address and port")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            #[derive(Debug, Deserialize)]
            struct DeserPeer {
                ip: String,
                port: u16,
                #[serde(rename = "peer id")]
                peer_id: String,
            }
            let mut peers = vec![];
            while let Ok(Some(peer)) = seq.next_element::<DeserPeer>() {
                println!("{:?}", peer);
                let ip = peer.ip.parse::<IpAddr>().map_err(|e| serde::de::Error::custom(e.to_string()))?;
                peers.push(Peer {
                    ip: SocketAddr::new(ip, peer.port),
                    peer_id: peer.peer_id,
                });
            }
            Ok(PeersKind::Objects(peers))
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            panic!("map {:?}", map.next_entry::<String, String>())
        }

        fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E> {
            Ok(PeersKind::Bytes(Peers(
                v.chunks_exact(6)
                    .map(|c| {
                        SocketAddrV4::new(
                            Ipv4Addr::new(c[0], c[1], c[2], c[3]),
                            u16::from_be_bytes(c[4..].try_into().expect("6 byte chunks")),
                        )
                    })
                    .collect::<Vec<SocketAddrV4>>(),
            )))
        }
    }

    deserializer.deserialize_seq(PeersVisitor)
}

pub(crate) fn urlencode(t: &[u8; 20]) -> String {
    let mut encoded = String::with_capacity(3 * t.len());
    for &byte in t {
        encoded.push('%');
        encoded.push_str(&hex::encode([byte]));
    }
    encoded
}
