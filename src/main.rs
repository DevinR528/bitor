#![feature(stmt_expr_attributes, try_trait_v2)]
#![allow(dead_code, unused_variables)]

use std::{
    collections::BTreeSet,
    convert::Infallible,
    fs,
    io::Write,
    net::{SocketAddr, SocketAddrV4},
    ops,
    path::{Path, PathBuf},
};

use anyhow::Context;
use clap::{Parser, Subcommand};
use serde_bencode as bc;
use serde_urlencoded as surl;
use sha1::{Digest, Sha1};

mod download;
mod torrent;
mod tracker;
mod utils;

use download::{Message, MessageKind, TcpTorrentClient, CHUNK_MAX};
use torrent::Torrent;
use tracker::{Tracker, TrackerResponse};
use utils::bencode_strings;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
#[clap(rename_all = "snake_case")]
enum Command {
    Decode {
        value: String,
    },
    Info {
        torrent: PathBuf,
    },
    #[clap(name = "peers")]
    DiscoverPeers {
        torrent: PathBuf,
    },
    Handshake {
        torrent: PathBuf,
        peer: String,
    },
    DownloadPiece {
        #[arg(short)]
        output: PathBuf,
        torrent: PathBuf,
        piece: usize,
    },
    Download {
        #[arg(short)]
        output: PathBuf,
        torrent: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match &args.command {
        Command::Decode { value } => {
            let res: bc::value::Value = bc::from_str(value).expect("Valid bencode data");
            let json_str = bencode_strings(&res);
            println!("{}", serde_json::to_string_pretty(&json_str).unwrap());
        }
        Command::Info { torrent } => {
            let tor = make_torrent(torrent).await?;

            println!("Tracker Url: {}", tor.announce);
            println!("Length: {}", tor.info.file_len());

            println!("Info Hash: {}", tor.info.hex_sha1());

            println!("Piece Hashes:");
            for chunk in &tor.info.pieces.0 {
                println!("{}", hex::encode(chunk));
            }
        }
        Command::DiscoverPeers { torrent } => {
            let tor = make_torrent(torrent).await?;

            let trk_resp = discover_request(&tor).await?;
            println!(
                "interval: {} peers: {:?}",
                trk_resp.interval, trk_resp.peers
            );
            for ip in trk_resp.peers.socket_addr() {
                println!("{}", ip);
            }
        }
        Command::Handshake { torrent, peer } => {
            let tor = make_torrent(torrent).await?;
            let addr = peer
                .parse::<SocketAddrV4>()
                .with_context(|| format!("failed to parse {} as socket address", peer))?;
            let mut tcp_client = TcpTorrentClient::new(&SocketAddr::V4(addr)).await?;

            let buf = tcp_client
                .handshake(&tor.info.sha1(), b"00112233445566778899")
                .await?;

            println!("Peer ID: {}", hex::encode(&buf[(1 + 19 + 8 + 20)..]));
            println!(
                "Peer ID: {}",
                String::from_utf8_lossy(&buf[(1 + 19 + 8 + 20)..])
            );
        }
        Command::DownloadPiece {
            output,
            torrent,
            piece,
        } => {
            let piece_idx = *piece as u32;
            let tor = make_torrent(torrent).await?;
            let tracker_resp = discover_request(&tor).await?;

            println!("{:?}", tor);
            println!("{:?}", tracker_resp);

            let addrs = tracker_resp.peers.socket_addr();
            let psudo_rng = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as usize;
            let addr = addrs[psudo_rng % addrs.len()];

            // Find the hash for this piece
            let hash = tor.info.pieces[piece_idx as usize];

            // Gather all the information about the piece
            let last_piece = (piece_idx as usize) == (tor.info.pieces.0.len() - 1);
            let piece_len = tor.info.piece_len as u32;
            let file_len = tor.info.file_len() as u32;
            let info = DownloadInfo {
                piece_idx,
                piece_len,
                last_piece,
                file_len,
                hash,
            };

            match download_piece(info, addr).await {
                DownloadResult::Ok { piece, bytes } => {
                    // write the collected file to the suggested place
                tokio::fs::write(&tor.info.name, &bytes).await?;

                println!(
                    "Piece {} downloaded to {}",
                    piece_idx,
                    output.to_string_lossy()
                );
                }
                DownloadResult::Retry { info, addr } => {
                    anyhow::bail!("failed to connect to peer {}, could retry", addr)
                }
                DownloadResult::Err(e) => anyhow::bail!(e),
            }
        }
        Command::Download { output, torrent } => {
            let tor = make_torrent(torrent).await?;
            let tracker_resp = discover_request(&tor).await?;
            let addrs = tracker_resp.peers.socket_addr();

            let mut piece_tasks = tokio::task::JoinSet::new();
            for piece_idx in 0..tor.info.pieces.len() {
                // Find the hash for this piece
                let hash = tor.info.pieces[piece_idx];

                // Pick an address for the peer
                let addr = addrs[piece_idx % addrs.len()];

                // Gather all the information about the piece
                let last_piece = piece_idx == (tor.info.pieces.0.len() - 1);
                let piece_len = tor.info.piece_len as u32;
                let file_len = tor.info.file_len() as u32;
                let info = DownloadInfo {
                    piece_idx: piece_idx as u32,
                    piece_len,
                    last_piece,
                    file_len,
                    hash,
                };

                // TODO: copying the hash everytime is NOT idea, lets see what we can do...
                piece_tasks.spawn(async move { download_piece(info, addr).await });
            }

            let mut all = BTreeSet::new();
            while let Some(res) = piece_tasks.join_next().await {
                match res.unwrap() {
                    DownloadResult::Ok { piece, bytes } => {
                        all.insert((piece, bytes));
                    }
                    DownloadResult::Retry { info, addr } => {
                        println!("RETRYING piece: {} from address: {}", info.piece_idx, addr);
                        let new_addr = *addrs.iter().find(|s| s != &&addr).unwrap();
                        piece_tasks
                            .spawn(async move { download_piece(info, new_addr).await });
                    }
                    DownloadResult::Err(e) => {
                        return Err(e);
                    }
                }
            }

            let mut fd = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .open(&tor.info.name)
                .with_context(|| format!("failed to open {}", tor.info.name))?;

            for (i, piece) in all {
                fd.write_all(&piece).with_context(|| {
                    format!("failed to write piece: {} to {}", i, tor.info.name)
                })?;
            }
        }
    }

    Ok(())
}

async fn discover_request(tor: &Torrent) -> anyhow::Result<TrackerResponse> {
    let len = tor.info.file_len();

    // The info_hash cannot be encoded by serde_urlencode or it prints them as u8 numbers or something...
    let trk = Tracker::new(len);
    let query = surl::to_string(trk).context("failed to serialize query params")?;

    let url = format!(
        "{}?{}&info_hash={}",
        tor.announce,
        query,
        tracker::urlencode(&tor.info.sha1())
    );
    // println!("{}", url);

    let bytes_res = reqwest::get(&url)
        .await
        .with_context(|| format!("failed to get request {}", url))?
        .bytes()
        .await?;
    bc::from_bytes::<TrackerResponse>(&bytes_res[..])
        .context("failed to deserialize TrackerResponse")
}

async fn make_torrent(file: &Path) -> anyhow::Result<Torrent> {
    let bytes = tokio::fs::read(file)
        .await
        .with_context(|| format!("failed to read file {}", file.to_string_lossy()))?;
    bc::from_bytes::<Torrent>(&bytes)
        .with_context(|| format!("failed to decode bencode data {}", file.to_string_lossy()))
}

enum DownloadResult {
    Ok {
        piece: u32,
        bytes: Vec<u8>,
    },
    Err(anyhow::Error),
    Retry {
        info: DownloadInfo,
        addr: SocketAddr,
    },
}

impl ops::FromResidual<Result<Infallible, anyhow::Error>> for DownloadResult {
    fn from_residual(residual: Result<Infallible, anyhow::Error>) -> Self {
        DownloadResult::Err(residual.expect_err("Infallible was fallible..."))
    }
}

struct DownloadInfo {
    piece_idx: u32,
    piece_len: u32,
    last_piece: bool,
    file_len: u32,
    hash: [u8; 20],
}

async fn download_piece(info: DownloadInfo, addr: SocketAddr) -> DownloadResult {
    let DownloadInfo {
        piece_idx,
        piece_len,
        last_piece,
        file_len,
        hash
    } = info;
    let mut tcp_stream = if let Ok(tcp) = TcpTorrentClient::new(&addr).await {
        tcp
    } else {
        return DownloadResult::Retry { info, addr };
    };
    let buf = if let Ok(b) = tcp_stream.handshake(&hash, b"00112233445566778899").await {
        b
    } else {
        return DownloadResult::Retry { info, addr };
    };

    println!("Peer ID: {}", hex::encode(&buf[(1 + 19 + 8 + 20)..]));
    println!("Peer ID: {:?}", &buf);

    let msg = tcp_stream.read_msg("bitfield").await?;
    println!("Bitfield msg: {:?}", msg);

    tcp_stream
        .write_msg(Message::interested())
        .await
        .context("send interested")?;

    println!("written interested message");

    let msg = tcp_stream.read_msg("interested response").await?;
    println!("unchoke msg: {:?}", msg);

    match msg.msg {
        download::MessageKind::Choke => {
            return DownloadResult::Err(anyhow::anyhow!(
                "peer returned choke after interest message"
            ));
        }
        download::MessageKind::Unchoke => {}
        download::MessageKind::Have => {
            // Possibly update bitfield and list of peers for the relevant pieces..
        }
        download::MessageKind::Request { .. }
        | download::MessageKind::Cancel
        | download::MessageKind::Interested
        | download::MessageKind::NotInterested => {
            // We keep trying I guess...
        }
        download::MessageKind::Bitfield { .. } => todo!(),
        download::MessageKind::Piece { .. } => {
            // Update that we no longer need to care about this piece...
        }
    }

    let mut whole_piece = Vec::with_capacity(piece_len as usize);
    #[rustfmt::skip]
    let piece_size = if last_piece {
        let left_over = file_len % piece_len;
        if left_over == 0 { piece_len } else { left_over }
    } else {
        piece_len
    };
    let num_chunks = (piece_size + (CHUNK_MAX - 1)) / CHUNK_MAX;
    println!(
        "{} + {} / {} = {}",
        piece_size,
        (CHUNK_MAX - 1),
        CHUNK_MAX,
        num_chunks
    );

    #[rustfmt::skip]
    for chunk in 0..num_chunks {
        let chunk_size = if chunk == num_chunks - 1 {
            let left_over = piece_size % CHUNK_MAX;
            if left_over == 0 { CHUNK_MAX } else { left_over }
        } else {
            CHUNK_MAX
        };

        println!(
            "index {} begining: {} len: {}",
            piece_idx,
            chunk * CHUNK_MAX,
            chunk_size
        );

        tcp_stream
            .write_msg(Message::request(piece_idx, chunk * CHUNK_MAX, chunk_size))
            .await?;
        let piece = tcp_stream.read_msg("piece").await?;
        println!("{:?}", piece);

        if let Message {msg: MessageKind::Piece { body, .. }, .. } = piece {
            whole_piece.extend_from_slice(&body);
        } else {
            return DownloadResult::Err(anyhow::anyhow!(
                "received invalid message '{}'", piece.name()
            ));
        }
    }

    let mut hasher = Sha1::new();
    hasher.update(&whole_piece);
    if hasher.finalize().as_ref() != hash {
        return DownloadResult::Err(anyhow::anyhow!(
            "piece {} hash did not match what peer sent",
            piece_idx
        ));
    }

    DownloadResult::Ok {
        piece: piece_idx,
        bytes: whole_piece,
    }
}
