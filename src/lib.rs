#[macro_use]
mod macros;

mod atomic_waker;

pub mod flags;

mod errors;

mod completion_port;
pub use self::completion_port::{
    CompletionOutcome, CompletionPoll, CompletionPort, CompletionStatus, RegisterOptions,
};

mod io;
pub use self::io::{Overlapped, OverlappedResult, OverlappedState};

mod arc_handle;
pub use self::arc_handle::ArcHandle;

mod task;

mod handle;
pub use self::handle::Handle;

mod socket;
pub use self::socket::Socket;

mod pool;
pub use self::pool::BufferPool;

pub mod pipe;

pub mod ioctl;

#[cfg(feature = "background")]
mod background;
#[cfg(feature = "background")]
pub use self::background::{setup, BackgroundThread};
