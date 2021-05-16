use crate::sys::{AsRawHandle, RawHandle};
use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::ptr;
use winapi::shared::minwindef::DWORD;
use winapi::um::handleapi;
use winapi::um::namedpipeapi;
use winapi::um::winbase;
use winapi::um::winnt::HANDLE;

/// A named pipe as created through [CreateNamedPipe].
#[derive(Debug)]
pub struct NamedPipe {
    handle: HANDLE,
}

// Safety: handles to named pipes are thread safe.
unsafe impl Send for NamedPipe {}
unsafe impl Sync for NamedPipe {}

impl Drop for NamedPipe {
    fn drop(&mut self) {
        unsafe {
            // NB: intentionally ignored.
            let _ = handleapi::CloseHandle(self.handle);
        }
    }
}

impl AsRawHandle for NamedPipe {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle as *mut _
    }
}

/// The pipe mode of a [NamedPipe].
///
/// Set through [CreatePipeOptions::pipe_mode].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PipeMode {
    /// Data is written to the pipe as a stream of bytes. The pipe does not
    /// distinguish bytes written during different write operations.
    ///
    /// Corresponds to [PIPE_TYPE_BYTE][winapi::um::winbase::PIPE_TYPE_BYTE].
    Byte,
    /// Data is written to the pipe as a stream of messages. The pipe treats the
    /// bytes written during each write operation as a message unit. Any reading
    /// function on [NamedPipe] returns [ERROR_MORE_DATA] when a message is not
    /// read completely.
    ///
    /// Corresponds to [PIPE_TYPE_MESSAGE][winapi::um::winbase::PIPE_TYPE_MESSAGE].
    ///
    /// [ERROR_MORE_DATA]: winapi::shared::winerror::ERROR_MORE_DATA
    Message,
}

// Helper to set a boolean flag as a bitfield.
macro_rules! bool_flag {
    ($f:expr, $t:expr, $flag:expr) => {{
        let current = $f;

        if $t {
            $f = current | $flag;
        } else {
            $f = current & !$flag;
        };
    }};
}

/// A builder structure for construct a named pipe with named pipe-specific
/// options. This is required to use for named pipe servers who wants to modify
/// pipe-related options.
///
/// See [CreatePipeOptions::create].
#[derive(Debug, Clone)]
pub struct CreatePipeOptions {
    open_mode: DWORD,
    pipe_mode: DWORD,
    max_instances: DWORD,
    out_buffer_size: DWORD,
    in_buffer_size: DWORD,
    default_timeout: DWORD,
    custom_flags: u32,
}

impl CreatePipeOptions {
    /// Creates a new named pipe builder with the default settings.
    ///
    /// ```
    /// use iocp::pipe::CreatePipeOptions;
    ///
    /// const PIPE_NAME: &str = r"\\.\pipe\iocp-new";
    ///
    /// # #[tokio::main] async fn main() -> std::io::Result<()> {
    /// let server = CreatePipeOptions::new().create(PIPE_NAME)?;
    /// # Ok(()) }
    /// ```
    pub fn new() -> CreatePipeOptions {
        CreatePipeOptions {
            open_mode: winbase::PIPE_ACCESS_DUPLEX,
            pipe_mode: winbase::PIPE_TYPE_BYTE | winbase::PIPE_REJECT_REMOTE_CLIENTS,
            max_instances: winbase::PIPE_UNLIMITED_INSTANCES,
            out_buffer_size: 65536,
            in_buffer_size: 65536,
            default_timeout: 0,
            custom_flags: 0,
        }
    }

    /// The pipe mode.
    ///
    /// The default pipe mode is [PipeMode::Byte]. See [PipeMode] for
    /// documentation of what each mode means.
    ///
    /// This corresponding to specifying [dwPipeMode].
    ///
    /// [dwPipeMode]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea
    pub fn pipe_mode(&mut self, pipe_mode: PipeMode) -> &mut Self {
        self.pipe_mode = match pipe_mode {
            PipeMode::Byte => winbase::PIPE_TYPE_BYTE,
            PipeMode::Message => winbase::PIPE_TYPE_MESSAGE,
        };

        self
    }

    /// The flow of data in the pipe goes from client to server only.
    ///
    /// This corresponds to setting [PIPE_ACCESS_INBOUND].
    ///
    /// [PIPE_ACCESS_INBOUND]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea#pipe_access_inbound
    ///
    /// # Examples
    ///
    /// ```
    /// use iocp::pipe::CreatePipeOptions;
    /// use std::fs::OpenOptions;
    /// use std::io;
    /// use std::os::windows::fs::OpenOptionsExt as _;
    /// use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    ///
    /// const PIPE_NAME: &str = r"\\.\pipe\iocp-access-inbound";
    ///
    /// # #[tokio::main] async fn main() -> io::Result<()> {
    /// let (port, handle) = iocp::setup(1)?;
    ///
    /// // Server side prevents connecting by denying inbound access, client errors
    /// // when attempting to open the connection.
    /// {
    ///     let _server = CreatePipeOptions::new()
    ///         .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
    ///         .access_inbound(false)
    ///         .create(PIPE_NAME)?;
    ///
    ///     let e = OpenOptions::new()
    ///         .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
    ///         .read(true)
    ///         .write(true)
    ///         .open(PIPE_NAME)
    ///         .unwrap_err();
    ///
    ///     assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    ///
    ///     // Disabling writing allows a client to connect, but leads to runtime
    ///     // error if a write is attempted.
    ///     let client = OpenOptions::new()
    ///         .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
    ///         .read(true)
    ///         .open(PIPE_NAME)?;
    ///
    ///     let mut client = port.register_handle(client, Default::default())?;
    ///
    ///     let e = client.write(b"ping").await.unwrap_err();
    ///     assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    /// }
    ///
    /// // A functional, unidirectional server-to-client only communication.
    /// {
    ///     let server = CreatePipeOptions::new()
    ///         .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
    ///         .access_inbound(false)
    ///         .create(PIPE_NAME)?;
    ///
    ///     let mut server = port.register_handle(server, Default::default())?;
    ///
    ///     let client = OpenOptions::new()
    ///         .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
    ///         .read(true)
    ///         .open(PIPE_NAME)?;
    ///
    ///     let mut client = port.register_handle(client, Default::default())?;
    ///
    ///     let write = server.write_all(b"ping");
    ///
    ///     let mut buf = [0u8; 4];
    ///     let read = client.read_exact(&mut buf);
    ///
    ///     let ((), read) = tokio::try_join!(write, read)?;
    ///
    ///     assert_eq!(read, 4);
    ///     assert_eq!(&buf[..], b"ping");
    /// }
    ///
    /// port.shutdown()?;
    /// handle.join()?;
    /// # Ok(()) }
    /// ```
    pub fn access_inbound(&mut self, allowed: bool) -> &mut Self {
        bool_flag!(self.open_mode, allowed, winbase::PIPE_ACCESS_INBOUND);
        self
    }

    /// The flow of data in the pipe goes from server to client only.
    ///
    /// This corresponds to setting [PIPE_ACCESS_OUTBOUND].
    ///
    /// [PIPE_ACCESS_OUTBOUND]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea#pipe_access_outbound
    ///
    /// # Examples
    ///
    /// ```
    /// use iocp::pipe::CreatePipeOptions;
    /// use std::fs::OpenOptions;
    /// use std::io;
    /// use std::os::windows::fs::OpenOptionsExt as _;
    /// use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    ///
    /// const PIPE_NAME: &str = r"\\.\pipe\iocp-access-outbound";
    ///
    /// # #[tokio::main] async fn main() -> io::Result<()> {
    /// let (port, handle) = iocp::setup(1)?;
    ///
    /// // Server side prevents connecting by denying outbound access, client errors
    /// // when attempting to open the connection.
    /// {
    ///     let _server = CreatePipeOptions::new()
    ///         .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
    ///         .access_outbound(false)
    ///         .create(PIPE_NAME)?;
    ///
    ///     let e = OpenOptions::new()
    ///         .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
    ///         .read(true)
    ///         .write(true)
    ///         .open(PIPE_NAME)
    ///         .unwrap_err();
    ///
    ///     assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    ///
    ///     // Disabling reading allows a client to connect, but leads to runtime
    ///     // error if a read is attempted.
    ///     let mut client = OpenOptions::new()
    ///         .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
    ///         .write(true)
    ///         .open(PIPE_NAME)?;
    ///
    ///     let mut client = port.register_handle(client, Default::default())?;
    ///
    ///     let mut buf = [0u8; 4];
    ///     let e = client.read(&mut buf).await.unwrap_err();
    ///     assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    /// }
    ///
    /// // A functional, unidirectional client-to-server only communication.
    /// {
    ///     let server = CreatePipeOptions::new()
    ///         .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
    ///         .access_outbound(false)
    ///         .create(PIPE_NAME)?;
    ///
    ///     let mut server = port.register_handle(server, Default::default())?;
    ///
    ///     let client = OpenOptions::new()
    ///         .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
    ///         .write(true)
    ///         .open(PIPE_NAME)?;
    ///
    ///     let mut client = port.register_handle(client, Default::default())?;
    ///
    ///     let write = client.write_all(b"ping");
    ///
    ///     let mut buf = [0u8; 4];
    ///     let read = server.read_exact(&mut buf);
    ///
    ///     let ((), read) = tokio::try_join!(write, read)?;
    ///
    ///     assert_eq!(read, 4);
    ///     assert_eq!(&buf[..], b"ping");
    /// }
    ///
    /// port.shutdown()?;
    /// handle.join()?;
    /// # Ok(()) }
    /// ```
    pub fn access_outbound(&mut self, allowed: bool) -> &mut Self {
        bool_flag!(self.open_mode, allowed, winbase::PIPE_ACCESS_OUTBOUND);
        self
    }

    /// If you attempt to open multiple instances of a pipe with this flag,
    /// creation of the first instance succeeds, but creation of the next
    /// instance fails with [ERROR_ACCESS_DENIED].
    ///
    /// This corresponds to setting [FILE_FLAG_FIRST_PIPE_INSTANCE].
    ///
    /// [ERROR_ACCESS_DENIED]: winapi::shared::winerror::ERROR_ACCESS_DENIED
    /// [FILE_FLAG_FIRST_PIPE_INSTANCE]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea#pipe_first_pipe_instance
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io;
    /// use iocp::pipe::CreatePipeOptions;
    ///
    /// const PIPE_NAME: &str = r"\\.\pipe\iocp-first-instance";
    ///
    /// # #[tokio::main] async fn main() -> io::Result<()> {
    /// let mut builder = CreatePipeOptions::new();
    /// builder.custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED);
    /// builder.first_pipe_instance(true);
    ///
    /// let server = builder.create(PIPE_NAME)?;
    /// let e = builder.create(PIPE_NAME).unwrap_err();
    /// assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    /// drop(server);
    ///
    /// // OK: since, we've closed the other instance.
    /// let _server2 = builder.create(PIPE_NAME)?;
    /// # Ok(()) }
    /// ```
    pub fn first_pipe_instance(&mut self, first: bool) -> &mut Self {
        bool_flag!(
            self.open_mode,
            first,
            winbase::FILE_FLAG_FIRST_PIPE_INSTANCE
        );
        self
    }

    /// Indicates whether this server can accept remote clients or not. This is
    /// enabled by default.
    ///
    /// This corresponds to setting [PIPE_REJECT_REMOTE_CLIENTS].
    ///
    /// [PIPE_REJECT_REMOTE_CLIENTS]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea#pipe_reject_remote_clients
    pub fn reject_remote_clients(&mut self, reject: bool) -> &mut Self {
        bool_flag!(self.pipe_mode, reject, winbase::PIPE_REJECT_REMOTE_CLIENTS);
        self
    }

    /// The maximum number of instances that can be created for this pipe. The
    /// first instance of the pipe can specify this value; the same number must
    /// be specified for other instances of the pipe. Acceptable values are in
    /// the range 1 through 254. The default value is unlimited.
    ///
    /// This corresponds to specifying [nMaxInstances].
    ///
    /// [nMaxInstances]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea
    /// [PIPE_UNLIMITED_INSTANCES]: winapi::um::winbase::PIPE_UNLIMITED_INSTANCES
    ///
    /// # Panics
    ///
    /// This function will panic if more than 254 instances are specified. If
    /// you do not wish to set an instance limit, leave it unspecified.
    ///
    /// ```should_panic
    /// use iocp::pipe::CreatePipeOptions;
    ///
    /// # #[tokio::main] async fn main() -> std::io::Result<()> {
    /// let builder = CreatePipeOptions::new().max_instances(255);
    /// # Ok(()) }
    /// ```
    pub fn max_instances(&mut self, instances: usize) -> &mut Self {
        assert!(instances < 255, "cannot specify more than 254 instances");
        self.max_instances = instances as DWORD;
        self
    }

    /// The number of bytes to reserve for the output buffer.
    ///
    /// This corresponds to specifying [nOutBufferSize].
    ///
    /// [nOutBufferSize]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea
    pub fn out_buffer_size(&mut self, buffer: u32) -> &mut Self {
        self.out_buffer_size = buffer as DWORD;
        self
    }

    /// The number of bytes to reserve for the input buffer.
    ///
    /// This corresponds to specifying [nInBufferSize].
    ///
    /// [nInBufferSize]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea
    pub fn in_buffer_size(&mut self, buffer: u32) -> &mut Self {
        self.in_buffer_size = buffer as DWORD;
        self
    }

    /// Create the named pipe identified by `addr` for use as a server.
    ///
    /// This function will call the [CreateNamedPipe] function and return the
    /// result.
    ///
    /// [CreateNamedPipe]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea
    ///
    /// # Errors
    ///
    /// This errors if called outside of a [Tokio Runtime] which doesn't have
    /// [I/O enabled] or if any OS-specific I/O errors occur.
    ///
    /// [Tokio Runtime]: tokio::runtime::Runtime
    /// [I/O enabled]: tokio::runtime::Builder::enable_io
    ///
    /// # Examples
    ///
    /// ```
    /// use iocp::pipe::CreatePipeOptions;
    ///
    /// const PIPE_NAME: &str = r"\\.\pipe\iocp-open";
    ///
    /// # #[tokio::main] async fn main() -> std::io::Result<()> {
    /// let server = CreatePipeOptions::new()
    ///     .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
    ///     .create(PIPE_NAME)?;
    /// # Ok(()) }
    /// ```
    pub fn create(&self, addr: impl AsRef<OsStr>) -> io::Result<NamedPipe> {
        // Safety: We're calling create_with_security_attributes w/ a null
        // pointer which disables it.
        unsafe { self.create_with_security_attributes(addr, ptr::null_mut()) }
    }

    /// Sets extra flags for the `dwOpenMode` argument to the call to
    /// [CreateNamedPipe] to the specified value.
    ///
    /// [CreateNamedPipe]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea
    pub fn custom_flags(&mut self, flags: u32) -> &mut Self {
        self.custom_flags = flags;
        self
    }

    /// Create the named pipe identified by `addr` for use as a server.
    ///
    /// This is the same as [open][CreatePipeOptions::create] except that it
    /// supports providing security attributes.
    ///
    /// # Errors
    ///
    /// This errors if called outside of a [Tokio Runtime] which doesn't have
    /// [I/O enabled] or if any OS-specific I/O errors occur.
    ///
    /// [Tokio Runtime]: tokio::runtime::Runtime
    /// [I/O enabled]: tokio::runtime::Builder::enable_io
    ///
    /// # Safety
    ///
    /// The caller must ensure that `attrs` points to an initialized instance of
    /// a [SECURITY_ATTRIBUTES] structure.
    ///
    /// [SECURITY_ATTRIBUTES]: [winapi::um::minwinbase::SECURITY_ATTRIBUTES]
    pub unsafe fn create_with_security_attributes(
        &self,
        addr: impl AsRef<OsStr>,
        attrs: *mut (),
    ) -> io::Result<NamedPipe> {
        let addr = encode_addr(addr);

        let handle = namedpipeapi::CreateNamedPipeW(
            addr.as_ptr(),
            self.custom_flags | self.open_mode,
            self.pipe_mode,
            self.max_instances,
            self.out_buffer_size,
            self.in_buffer_size,
            self.default_timeout,
            attrs as *mut _,
        );

        if handle == handleapi::INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        Ok(NamedPipe { handle })
    }
}

/// Encode an address so that it is a null-terminated wide string.
fn encode_addr(addr: impl AsRef<OsStr>) -> Box<[u16]> {
    addr.as_ref().encode_wide().chain(Some(0)).collect()
}
