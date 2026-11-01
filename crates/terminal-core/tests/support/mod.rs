//! Raw session daemon frames, shared by the daemon tests and the pinned
//! protocol fixtures. The frame layout and the hello are the same in every
//! protocol version, so a client can tell which version a daemon speaks.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// One frame: a little-endian `u32` length, a kind byte and its payload.
pub struct Frame(Vec<u8>);

impl Frame {
    pub fn new(kind: u8) -> Self {
        Self(vec![kind])
    }
    pub fn u8(mut self, value: u8) -> Self {
        self.0.push(value);
        self
    }
    pub fn u16(mut self, value: u16) -> Self {
        self.0.extend(value.to_le_bytes());
        self
    }
    pub fn u32(mut self, value: u32) -> Self {
        self.0.extend(value.to_le_bytes());
        self
    }
    pub fn u64(mut self, value: u64) -> Self {
        self.0.extend(value.to_le_bytes());
        self
    }
    pub fn raw(mut self, value: &[u8]) -> Self {
        self.0.extend(value);
        self
    }
    /// A length-prefixed byte string.
    pub fn bytes(self, value: &[u8]) -> Self {
        self.u32(value.len() as u32).raw(value)
    }
    pub fn str(self, value: &str) -> Self {
        self.bytes(value.as_bytes())
    }
    pub fn done(self) -> Vec<u8> {
        let mut frame = (self.0.len() as u32).to_le_bytes().to_vec();
        frame.extend(self.0);
        frame
    }
}

/// The hello that opens every connection: the magic, then the version.
pub fn hello(version: u32) -> Vec<u8> {
    Frame::new(1).raw(b"NEPTUNE-SESSIONS").u32(version).done()
}

pub fn welcome(version: u32) -> Vec<u8> {
    Frame::new(64).u32(version).done()
}

/// The daemon's answer to a hello of a version it doesn't speak.
pub fn refused(version: u32) -> Vec<u8> {
    Frame::new(65).u32(version).done()
}

/// Where the daemon of protocol `version` serving `dir` listens.
pub fn socket(dir: &Path, version: u32) -> PathBuf {
    dir.join(format!("v{version}"))
}
