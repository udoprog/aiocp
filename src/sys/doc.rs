use crate::completion_port::{CompletionPoll, CompletionStatus};
use crate::overlapped_handle::OverlappedHandle;
use std::io;
use std::marker;

pub struct RawHandle(());

pub trait AsRawHandle {
    fn as_raw_handle(&self) -> RawHandle {
        unreachable!()
    }
}

#[derive(Debug)]
pub(crate) struct CompletionPort(());

impl CompletionPort {
    pub(crate) fn create(threads: u32) -> io::Result<Self> {
        unreachable!()
    }

    pub(crate) fn permit(&self) -> io::Result<CompletionPortPermit<'_>> {
        unreachable!()
    }

    pub(crate) fn register<H>(&self, handle: H, key: usize) -> io::Result<OverlappedHandle<H>>
    where
        H: AsRawHandle,
    {
        unreachable!()
    }

    pub(crate) fn post(&self, completion_port: usize, overlapped: *mut ()) -> io::Result<()> {
        unreachable!()
    }

    pub(crate) fn poll(&self) -> io::Result<CompletionPoll> {
        unreachable!()
    }

    pub(crate) fn poll_during_shutdown(&self) -> io::Result<CompletionStatus> {
        unreachable!()
    }

    pub(crate) fn shutdown(&self) -> io::Result<()> {
        unreachable!()
    }
}

pub(crate) struct CompletionPortPermit<'a>(marker::PhantomData<&'a CompletionPort>);
