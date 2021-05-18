use std::os::windows::io::{AsRawHandle, RawHandle};
use std::sync::Arc;

/// Wraps the interior type `H` which implements [AsRawHandle] in an atomically
/// reference counted object allowing it to be cheaply cloned.
#[derive(Debug)]
pub struct ArcHandle<H>
where
    H: AsRawHandle,
{
    handle: Arc<H>,
}

impl<H> ArcHandle<H>
where
    H: AsRawHandle,
{
    /// Constructs a new [ArcHandle].
    pub fn new(handle: H) -> Self {
        Self {
            handle: Arc::new(handle),
        }
    }
}

impl<H> AsRawHandle for ArcHandle<H>
where
    H: AsRawHandle,
{
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.as_raw_handle()
    }
}

impl<H> Clone for ArcHandle<H>
where
    H: AsRawHandle,
{
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
        }
    }
}
