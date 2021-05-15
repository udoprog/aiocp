#[macro_use]
mod macros;

#[cfg(not(docsrs))]
#[path = "sys/sys.rs"]
mod sys;

#[cfg(docsrs)]
#[path = "sys/doc.rs"]
mod sys;

mod atomic_waker;

pub mod flags;

mod errors;

mod ext;
pub use self::ext::HandleExt;

mod completion_port;
pub use self::completion_port::{
    CompletionOutcome, CompletionPoll, CompletionPort, CompletionStatus,
};

mod io;
pub use self::io::Overlapped;

mod operation;
pub use self::operation::Operation;

mod overlapped_handle;
pub use self::overlapped_handle::{OverlappedHandle, OverlappedResult};

pub mod ops;

mod pool;
pub use self::pool::BufferPool;

mod handle;
pub use self::handle::Handle;

pub mod pipe;

pub mod ioctl;

pub mod tokio;

#[cfg(feature = "background")]
mod background;
#[cfg(feature = "background")]
pub use self::background::{setup, BackgroundThread};
