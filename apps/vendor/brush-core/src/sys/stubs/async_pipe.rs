//! Async pipe reading utilities for non-Unix platforms.
//!
//! Uses `spawn_blocking` internally for the I/O operation only,
//! not for the entire subshell execution.

use std::io::{self, Read};

pub(crate) struct AsyncPipeReader {
    inner: Option<crate::sys::wasm::pipes::PipeReader>,
}

impl AsyncPipeReader {
    pub(crate) fn new(fd: crate::sys::wasm::pipes::PipeReader) -> io::Result<Self> {
        Ok(Self { inner: Some(fd) })
    }

    pub(crate) async fn read_to_string(&mut self) -> io::Result<String> {
        let Some(reader) = self.inner.take() else {
            return Ok(String::new());
        };

        let mut reader = reader;
        let mut out = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => tokio::task::yield_now().await,
                Err(e) => return Err(e),
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}
