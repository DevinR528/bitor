use std::{fmt, net::SocketAddr};

use anyhow::Context;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

pub const CHUNK_MAX: u32 = 1 << 14;

#[derive(Debug)]
#[repr(u8)]
pub(crate) enum MessageTag {
    /// After interested message choke is returned until peer is ready.
    Choke = 0,
    /// Unchoke is sent once peer is ready after interested message.
    Unchoke = 1,
    /// Sent to indicate interest in a file.
    Interested = 2,
    /// Sent to indicate client is not interested in a file.
    NotInterested = 3,
    /// Sent to indicate client has piece.
    Have = 4,
    /// A bitfield representing the pieces this peer has.
    Bitfield = 5,
    /// Sent to request a specific piece at a specific byte offset.
    Request = 6,
    /// Sent to request a specific piece.
    Piece = 7,
    /// Sent to cancel a download.
    Cancel = 8,
}

pub(crate) enum MessageKind {
    /// After interested message choke is returned until peer is ready.
    Choke,
    /// Unchoke is sent once peer is ready after interested message.
    Unchoke,
    /// Sent to indicate interest in a file.
    Interested,
    /// Sent to indicate client is not interested in a file.
    NotInterested,
    /// Sent to indicate client has piece.
    Have,
    /// A bitfield representing the pieces this peer has.
    Bitfield { field: Vec<u8> },
    /// Sent to request a specific piece at a specific byte offset.
    Request { index: u32, begin: u32, len: u32 },
    /// Sent to request a specific piece.
    Piece {
        index: u32,
        begin: u32,
        body: Vec<u8>,
    },
    /// Sent to cancel a download.
    Cancel,
}

#[derive(Debug)]
pub struct Message {
    len: u32,
    pub msg: MessageKind,
}

#[derive(Debug)]
pub(crate) struct TcpTorrentClient {
    stream: TcpStream,
}

impl TcpTorrentClient {
    pub async fn new(addr: &SocketAddr) -> anyhow::Result<Self> {
        Ok(Self {
            stream: tokio::net::TcpStream::connect(addr)
                .await
                .with_context(|| format!("failed to connect to {}", addr))?,
        })
    }

    pub async fn handshake(&mut self, info_hash: &[u8], id: &[u8]) -> anyhow::Result<[u8; 68]> {
        let mut bytes: Vec<u8> = vec![19];
        bytes.extend_from_slice(b"BitTorrent protocol");
        bytes.extend_from_slice(&[0u8; 8]);
        bytes.extend_from_slice(info_hash);
        bytes.extend_from_slice(id);

        self.stream
            .write_all(&bytes)
            .await
            .context("write all handshake bytes")?;

        let mut buf = [0u8; 1 + 19 + 8 + 20 + 20];
        self.stream
            .read_exact(&mut buf)
            .await
            .context("read all handshake bytes")?;

        Ok(buf)
    }

    pub async fn read_msg(&mut self, kind: &str) -> anyhow::Result<Message> {
        let mut msg_len = 0;
        while msg_len == 0 {
            let mut buf = [0u8; 4];
            self.stream
                .read_exact(&mut buf)
                .await
                .with_context(|| format!("read len bytes of {}", kind))?;
            msg_len = u32::from_be_bytes(buf);
        }

        let mut buf = vec![0u8; msg_len as usize];
        self.stream
            .read_exact(&mut buf)
            .await
            .with_context(|| format!("read message bytes of {}", kind))?;
        Message::new(msg_len, buf)
    }

    pub async fn write_msg(&mut self, msg: Message) -> anyhow::Result<()> {
        self.stream
            .write_all(&msg.as_bytes())
            .await
            .with_context(|| format!("read message bytes of {}", msg.name()))
    }
}

impl Message {
    pub fn new(len: u32, buf: Vec<u8>) -> anyhow::Result<Self> {
        Ok(Self {
            len,
            msg: MessageKind::new(&buf)?,
        })
    }
    pub fn interested() -> Self {
        Self {
            len: 0,
            msg: MessageKind::Interested,
        }
    }
    pub fn request(index: u32, begin: u32, len: u32) -> Self {
        Self {
            len: 0,
            msg: MessageKind::Request { index, begin, len },
        }
    }

    pub fn name(&self) -> &str {
        match self.msg {
            MessageKind::Choke => "choke",
            MessageKind::Unchoke => "unchoke",
            MessageKind::Interested => "interested",
            MessageKind::NotInterested => "notinterested",
            MessageKind::Have => "have",
            MessageKind::Bitfield { .. } => "bitfield",
            MessageKind::Request { .. } => "request",
            MessageKind::Piece { .. } => "piece",
            MessageKind::Cancel => "cancel",
        }
    }

    pub fn as_bytes(&self) -> Vec<u8> {
        let mut bytes = u32::to_be_bytes(self.len + 1).to_vec();
        match &self.msg {
            MessageKind::Choke => {
                bytes.push(MessageTag::Choke as u8);
            }
            MessageKind::Unchoke => {
                bytes.push(MessageTag::Unchoke as u8);
            }
            MessageKind::Interested => {
                bytes.push(MessageTag::Interested as u8);
            }
            MessageKind::NotInterested => {
                bytes.push(MessageTag::NotInterested as u8);
            }
            MessageKind::Have => {
                bytes.push(MessageTag::Have as u8);
            }
            MessageKind::Bitfield { field } => {
                bytes.push(MessageTag::Bitfield as u8);
                bytes.extend_from_slice(field);
            }
            MessageKind::Request { index, begin, len } => {
                bytes.push(MessageTag::Request as u8);
                bytes.extend_from_slice(&index.to_be_bytes());
                bytes.extend_from_slice(&begin.to_be_bytes());
                bytes.extend_from_slice(&len.to_be_bytes());
            }
            MessageKind::Piece { index, begin, body } => {
                bytes.push(MessageTag::Piece as u8);
                bytes.extend_from_slice(&index.to_be_bytes());
                bytes.extend_from_slice(&begin.to_be_bytes());
                bytes.extend_from_slice(body);
            }
            MessageKind::Cancel => {
                bytes.push(MessageTag::Cancel as u8);
            }
        }
        bytes
    }
}

impl MessageKind {
    pub fn new(buf: &[u8]) -> anyhow::Result<Self> {
        Ok(match &buf[0] {
            0 => MessageKind::Choke,
            1 => MessageKind::Unchoke,
            2 => MessageKind::Interested,
            3 => MessageKind::NotInterested,
            4 => MessageKind::Have,
            5 => MessageKind::Bitfield {
                field: buf[1..].to_vec(),
            },
            6 => MessageKind::Request {
                index: u32::from_be_bytes(buf[1..5].try_into().context("context")?),
                begin: u32::from_be_bytes(buf[5..9].try_into().context("context")?),
                len: u32::from_be_bytes(buf[9..13].try_into().context("context")?),
            },
            7 => MessageKind::Piece {
                index: u32::from_be_bytes(buf[1..5].try_into().context("context")?),
                begin: u32::from_be_bytes(buf[5..9].try_into().context("context")?),
                body: buf[9..].to_vec(),
            },
            8 => MessageKind::Cancel,
            _ => anyhow::bail!("invalid MessageKind"),
        })
    }
}

impl fmt::Debug for MessageKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MessageKind::Choke => fmt::Formatter::write_str(f, "Choke"),
            MessageKind::Unchoke => fmt::Formatter::write_str(f, "Unchoke"),
            MessageKind::Interested => fmt::Formatter::write_str(f, "Interested"),
            MessageKind::NotInterested => fmt::Formatter::write_str(f, "NotInterested"),
            MessageKind::Have => fmt::Formatter::write_str(f, "Have"),
            MessageKind::Bitfield { field: __self_0 } => f
                .debug_struct("Bitfield")
                .field("field", &__self_0)
                .finish(),
            MessageKind::Request {
                index: __self_0,
                begin: __self_1,
                len: __self_2,
            } => f
                .debug_struct("Request")
                .field("index", &__self_0)
                .field("begin", &__self_1)
                .field("len", &__self_2)
                .finish(),
            MessageKind::Piece {
                index: __self_0,
                begin: __self_1,
                body: __self_2,
            } => f
                .debug_struct("Request")
                .field("index", &__self_0)
                .field("begin", &__self_1)
                .field("body", &&__self_2[..10])
                .finish(),
            MessageKind::Cancel => fmt::Formatter::write_str(f, "Cancel"),
        }
    }
}

#[test]
fn message_kind_request() {
    let req = MessageKind::new(&[6u8, 0, 0, 0, 3, 0, 0, 0, 7, 0, 0, 2, 154]).unwrap();
    if let MessageKind::Request { index, begin, len } = req {
        assert!(index == 3);
        assert!(begin == 7);
        assert!(len == 666);
    } else {
        panic!("MessageKind did from bytes did not create Request variant")
    }
}
