//! Signal handling for MavexOS: signals are delivered by the host scheduler (Linux numbering).

use crate::{error, sys, traps};

macro_rules! signals {
    ($($name:ident = $n:expr, $s:expr;)*) => {
        /// System signals (Linux numbering).
        #[allow(clippy::upper_case_acronyms)]
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        #[repr(i32)]
        pub enum Signal { $($name = $n),* }

        impl Signal {
            /// Returns an iterator over all possible signals.
            pub fn iterator() -> impl Iterator<Item = Self> {
                const ALL: &[Signal] = &[$(Signal::$name),*];
                ALL.iter().copied()
            }

            /// Converts the signal into its name.
            pub const fn as_str(self) -> &'static str {
                match self { $(Signal::$name => $s),* }
            }

            /// Creates a `Signal` from its name.
            pub fn from_str(s: &str) -> Result<Self, error::Error> {
                match s { $($s => Ok(Signal::$name),)* _ => Err(error::ErrorKind::InvalidSignal(s.into()).into()) }
            }
        }

        impl TryFrom<i32> for Signal {
            type Error = error::Error;
            fn try_from(value: i32) -> Result<Self, Self::Error> {
                match value { $($n => Ok(Signal::$name),)* _ => Err(error::ErrorKind::InvalidSignal(std::format!("{value}")).into()) }
            }
        }
    };
}

signals! {
    SIGHUP = 1, "SIGHUP"; SIGINT = 2, "SIGINT"; SIGQUIT = 3, "SIGQUIT"; SIGILL = 4, "SIGILL";
    SIGTRAP = 5, "SIGTRAP"; SIGABRT = 6, "SIGABRT"; SIGBUS = 7, "SIGBUS"; SIGFPE = 8, "SIGFPE";
    SIGKILL = 9, "SIGKILL"; SIGUSR1 = 10, "SIGUSR1"; SIGSEGV = 11, "SIGSEGV"; SIGUSR2 = 12, "SIGUSR2";
    SIGPIPE = 13, "SIGPIPE"; SIGALRM = 14, "SIGALRM"; SIGTERM = 15, "SIGTERM"; SIGCHLD = 17, "SIGCHLD";
    SIGCONT = 18, "SIGCONT"; SIGSTOP = 19, "SIGSTOP"; SIGTSTP = 20, "SIGTSTP"; SIGTTIN = 21, "SIGTTIN";
    SIGTTOU = 22, "SIGTTOU"; SIGURG = 23, "SIGURG"; SIGXCPU = 24, "SIGXCPU"; SIGXFSZ = 25, "SIGXFSZ";
    SIGVTALRM = 26, "SIGVTALRM"; SIGPROF = 27, "SIGPROF"; SIGWINCH = 28, "SIGWINCH"; SIGIO = 29, "SIGIO";
    SIGPWR = 30, "SIGPWR"; SIGSYS = 31, "SIGSYS";
}

fn send(pid: sys::process::ProcessId, sig: i32) -> Result<(), error::Error> {
    if crate::sys::wasm::mavex::send_signal(pid, sig) < 0 {
        return Err(error::ErrorKind::FailedToSendSignal.into());
    }
    Ok(())
}

pub(crate) fn continue_process(pid: sys::process::ProcessId) -> Result<(), error::Error> {
    // A job's pid is also its process group id; signal the whole group.
    send(-pid, Signal::SIGCONT as i32)
}

/// Sends a signal to a specific process.
pub fn kill_process(pid: sys::process::ProcessId, signal: traps::TrapSignal) -> Result<(), error::Error> {
    match signal {
        traps::TrapSignal::Signal(s) => send(pid, s as i32),
        other => Err(error::ErrorKind::InvalidSignal(other.to_string()).into()),
    }
}

pub(crate) fn lead_new_process_group() -> Result<(), error::Error> {
    Ok(())
}

/// A future that resolves when a stopped child is waiting to be reported.
pub(crate) struct ChildEvent;

impl ChildEvent {
    pub async fn recv(&self) {
        loop {
            if crate::sys::wasm::mavex::has_unreported_stop() {
                return;
            }
            tokio::task::yield_now().await;
        }
    }
}

pub(crate) fn tstp_signal_listener() -> Result<NeverSignal, error::Error> {
    Ok(NeverSignal)
}

pub(crate) struct NeverSignal;

impl NeverSignal {
    pub async fn recv(&self) {
        futures::future::pending::<()>().await;
    }
}

pub(crate) fn chld_signal_listener() -> Result<ChildEvent, error::Error> {
    Ok(ChildEvent)
}

pub(crate) async fn await_ctrl_c() -> std::io::Result<()> {
    NeverSignal.recv().await;
    Ok(())
}

pub(crate) fn mask_sigttou() -> Result<(), error::Error> {
    Ok(())
}

pub(crate) fn poll_for_stopped_children() -> Result<bool, error::Error> {
    Ok(crate::sys::wasm::mavex::take_unreported_stop())
}
