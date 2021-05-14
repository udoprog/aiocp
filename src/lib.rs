#[macro_use]
mod macros;

mod atomic_waker;

pub mod flags;

mod ext;
pub use self::ext::HandleExt;

mod completion_port;
pub use self::completion_port::{
    CompletionOutcome, CompletionPoll, CompletionPort, CompletionStatus,
};

mod io;
pub use self::io::{Overlapped, OverlappedResult};

mod operation;
pub use self::operation::Operation;

mod iocp_handle;
pub use self::iocp_handle::{IocpRun, OverlappedHandle};

mod buf;
pub use self::buf::IocpBuf;

pub mod ops;

mod pool;
pub use self::pool::IocpPool;

mod handle;
pub use self::handle::Handle;

mod pipe;
pub use self::pipe::{CreatePipeOptions, OpenOptions};

pub mod ioctl;

#[cfg(feature = "tokio")]
pub mod tokio;

#[cfg(feature = "background")]
mod background;
#[cfg(feature = "background")]
pub use self::background::{setup, BackgroundThread};
