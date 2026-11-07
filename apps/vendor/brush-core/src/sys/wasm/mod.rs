pub use crate::sys::stubs::async_pipe;
pub use crate::sys::stubs::commands;
pub(crate) use crate::sys::stubs::env;
pub use crate::sys::stubs::fd;
pub(crate) mod fs;
pub use crate::sys::stubs::input;
pub(crate) use crate::sys::stubs::network;
pub mod mavex;
pub(crate) mod pipes {
    pub(crate) use super::mavex::{PipeReader, PipeWriter};
    pub(crate) fn pipe() -> std::io::Result<(PipeReader, PipeWriter)> { super::mavex::make_pipe() }
}
pub use crate::sys::stubs::poll;
pub use crate::sys::stubs::process;
pub use crate::sys::stubs::resource;
pub use crate::sys::stubs::signal;
pub use crate::sys::stubs::terminal;
pub(crate) use crate::sys::stubs::users;

/// Platform-specific errors.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {}
