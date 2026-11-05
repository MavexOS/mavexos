//! Process management (MavexOS: children run to completion inside the host).

pub(crate) type ProcessId = i32;

#[derive(Clone, Copy, Debug)]
pub struct ExitStatus(pub i32);

impl ExitStatus {
    pub fn code(&self) -> Option<i32> { Some(self.0) }
    pub fn success(&self) -> bool { self.0 == 0 }
}

#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Provides access to a child process.
pub struct Child {
    pid: u32,
}

impl Child {
    pub fn new(pid: u32) -> Self { Self { pid } }

    pub fn id(&self) -> Option<u32> { Some(self.pid) }

    pub async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        // Let producer tasks (builtins, subshells) finish before the child reads its stdin.
        for _ in 0..4096 {
            if crate::sys::wasm::mavex::child_ready(self.pid) { break; }
            tokio::task::yield_now().await;
        }
        Ok(ExitStatus(crate::sys::wasm::mavex::run_child(self.pid)))
    }

    pub async fn wait_with_output(mut self) -> std::io::Result<Output> {
        let status = self.wait().await?;
        Ok(Output { status, stdout: Vec::new(), stderr: Vec::new() })
    }
}
