use std::{fmt, ops::Index};

use serde::{
    de::{self, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use serde_bencode as bc;
use sha1::{Digest, Sha1};

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct File {
    /// The length of this file in bytes.
    pub(crate) length: usize,
    /// List of subdirectory names with the last being the files path.
    pub(crate) path: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum FileType {
    Single { length: usize },
    Multi { files: Vec<File> },
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Info {
    /// A `length` if only one file is present or `files` if many.
    #[serde(flatten)]
    pub(crate) file: FileType,
    /// Suggested name to save the file / directory as.
    pub(crate) name: String,
    /// Number of bytes in each piece.
    #[serde(rename = "piece length")]
    pub(crate) piece_len: usize,
    /// 20 byte long SHA-1 hashes of each piece, this was assembled from the concatenation
    /// of them in the bencode blob.
    pub(crate) pieces: Hashes,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Torrent {
    /// URL to a "tracker", which is a central server that keeps track of peers
    /// participating in the sharing of a torrent.
    pub(crate) announce: String,
    /// Information about the file to be downloaded.
    pub(crate) info: Info,
}

#[derive(Debug)]
pub(crate) struct Hashes(pub Vec<[u8; 20]>);

impl Info {
    /// Make the SHA1 hash of the `Info`.
    pub(crate) fn sha1(&self) -> [u8; 20] {
        let mut hasher = Sha1::new();
        hasher.update(bc::to_bytes(self).expect("just deserialized Info struct"));
        hasher
            .finalize()
            .try_into()
            .expect("guaranteed to be a 20 byte array")
    }

    /// Hex encode the SHA1 hash of `Info``.
    pub(crate) fn hex_sha1(&self) -> String {
        hex::encode(self.sha1())
    }

    pub(crate) fn file_len(&self) -> usize {
        match &self.file {
            FileType::Single { length } => *length,
            FileType::Multi { files } => files.iter().map(|f| f.length).sum(),
        }
    }
}

impl Hashes {
    pub fn len(&self) -> usize { self.0.len() }
}

impl Index<usize> for Hashes {
    type Output = [u8; 20];

    fn index(&self, idx: usize) -> &Self::Output { &self.0[idx] }
}

struct HashesVisitor;
impl<'de> Visitor<'de> for HashesVisitor {
    type Value = Hashes;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a byte string whose length is a multiple of 20")
    }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if v.len() % 20 != 0 {
            return Err(E::custom(format!("length is {}", v.len())));
        }
        Ok(Hashes(
            v.chunks_exact(20)
                .map(|slice_20| slice_20.try_into().expect("guaranteed to be length 20"))
                .collect(),
        ))
    }
}
impl<'de> Deserialize<'de> for Hashes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_bytes(HashesVisitor)
    }
}
impl Serialize for Hashes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let single_slice = self.0.concat();
        serializer.serialize_bytes(&single_slice)
    }
}
