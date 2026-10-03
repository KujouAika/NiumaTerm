//! DNS-SD through the system mDNSResponder (Bonjour). The daemon owns UDP
//! 5353 and answers for every interface, so the calling process opens no
//! multicast socket of its own and runs no responder that would compete with
//! the system one for the port.
//!
//! Each operation talks to the daemon over a Unix socket. Its replies are
//! read on a runtime task that waits for the socket to become readable and
//! then calls `DNSServiceProcessResult`, which runs the callback on that same
//! task. The callback context therefore points at state the task owns, and
//! no reply crosses threads behind the runtime's back.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::net::Ipv4Addr;
use std::os::fd::RawFd;
use std::{ptr, slice};

use anyhow::{Context as _, Result, bail};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::AbortHandle;
use tracing::warn;

use crate::runtime;

/// An opaque `DNSServiceRef`.
type ServiceRef = *mut c_void;

/// Set on a browse reply for an instance that appeared, clear for one that
/// left.
const FLAGS_ADD: u32 = 0x2;

/// Set while the daemon has more replies queued for the same operation.
const FLAGS_MORE_COMING: u32 = 0x1;

const PROTOCOL_IPV4: u32 = 0x1;

type BrowseReplyFn = unsafe extern "C" fn(
    ServiceRef,
    u32,
    u32,
    i32,
    *const c_char,
    *const c_char,
    *const c_char,
    *mut c_void,
);

type ResolveReplyFn = unsafe extern "C" fn(
    ServiceRef,
    u32,
    u32,
    i32,
    *const c_char,
    *const c_char,
    u16,
    u16,
    *const u8,
    *mut c_void,
);

type AddressReplyFn = unsafe extern "C" fn(
    ServiceRef,
    u32,
    u32,
    i32,
    *const c_char,
    *const libc::sockaddr,
    u32,
    *mut c_void,
);

type RegisterReplyFn = unsafe extern "C" fn(
    ServiceRef,
    u32,
    i32,
    *const c_char,
    *const c_char,
    *const c_char,
    *mut c_void,
);

/// Bindings for dns_sd.h, which libSystem exports on every macOS version.
mod sys {
    use std::ffi::{c_char, c_int, c_void};

    use crate::unix::macos::dns_sd::{
        AddressReplyFn, BrowseReplyFn, RegisterReplyFn, ResolveReplyFn, ServiceRef,
    };

    unsafe extern "C" {
        pub(crate) fn DNSServiceRefSockFD(service: ServiceRef) -> c_int;

        pub(crate) fn DNSServiceProcessResult(service: ServiceRef) -> i32;

        pub(crate) fn DNSServiceRefDeallocate(service: ServiceRef);

        pub(crate) fn DNSServiceBrowse(
            service: *mut ServiceRef,
            flags: u32,
            interface: u32,
            regtype: *const c_char,
            domain: *const c_char,
            reply: Option<BrowseReplyFn>,
            context: *mut c_void,
        ) -> i32;

        pub(crate) fn DNSServiceResolve(
            service: *mut ServiceRef,
            flags: u32,
            interface: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            reply: Option<ResolveReplyFn>,
            context: *mut c_void,
        ) -> i32;

        pub(crate) fn DNSServiceGetAddrInfo(
            service: *mut ServiceRef,
            flags: u32,
            interface: u32,
            protocol: u32,
            host: *const c_char,
            reply: Option<AddressReplyFn>,
            context: *mut c_void,
        ) -> i32;

        pub(crate) fn DNSServiceRegister(
            service: *mut ServiceRef,
            flags: u32,
            interface: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            host: *const c_char,
            port: u16,
            txt_len: u16,
            txt: *const c_void,
            reply: Option<RegisterReplyFn>,
            context: *mut c_void,
        ) -> i32;

        pub(crate) fn DNSServiceUpdateRecord(
            service: ServiceRef,
            record: *mut c_void,
            flags: u32,
            rdata_len: u16,
            rdata: *const c_void,
            ttl: u32,
        ) -> i32;
    }
}

/// A running browse or resolve. Dropping it stops the operation.
pub struct Query {
    task: AbortHandle,
}

/// An instance appearing or leaving, as a browse reports it.
pub struct BrowseReply {
    /// Whether the instance appeared, as opposed to left.
    pub added: bool,

    /// The instance label, such as `Office Mac`.
    pub name: String,

    /// The service type with a trailing dot, such as `_http._tcp.`.
    pub regtype: String,

    /// The domain with a trailing dot, such as `local.`.
    pub domain: String,

    /// The interface the instance was seen on. One instance is reported
    /// once per interface it answers on, and leaves per interface too.
    pub interface: u32,
}

/// What a resolve returned.
pub struct ServiceInstance {
    /// The host the service runs on, such as `office-mac.local.`.
    pub host: String,

    pub port: u16,

    /// The TXT record as key and value pairs.
    pub properties: Vec<(String, String)>,
}

/// A service to publish.
pub struct ServiceRecord<'a> {
    /// The instance label, such as `Office Mac`.
    pub name: &'a str,

    /// The service type, such as `_http._tcp`.
    pub regtype: &'a str,

    pub port: u16,
    pub properties: &'a [(&'a str, &'a str)],
}

/// A published service. Dropping it withdraws the record at once.
pub struct Registration {
    service: ServiceRef,
}

/// A daemon operation with the state its callback writes to.
struct Operation<S> {
    service: ServiceRef,

    /// Taken before the operation is deallocated, which closes the socket
    /// the reactor still watches.
    socket: Option<AsyncFd<RawFd>>,

    /// The callback context. Boxed so its address holds while the operation
    /// moves.
    state: Box<S>,
}

// SAFETY: the daemon connection is used by one owner at a time, and the
// callback runs only inside `DNSServiceProcessResult` on that owner's task.
unsafe impl<S: Send> Send for Operation<S> {}

// SAFETY: the daemon connection is only used to update or withdraw the
// record, each a single request the client library completes before
// returning, and `Registration` is not `Sync`, so no two of those calls
// overlap.
unsafe impl Send for Registration {}

impl<S> Operation<S> {
    /// Start an operation: `begin` makes the `DNSService*` call with the
    /// reference to fill in and the callback context.
    fn start(
        state: S,
        call: &str,
        begin: impl FnOnce(*mut ServiceRef, *mut c_void) -> i32,
    ) -> Result<Self> {
        let mut state = Box::new(state);
        let mut service = ptr::null_mut();

        let code = begin(&mut service, (&raw mut *state).cast());

        if code != 0 {
            bail!("{call} failed with error {code}");
        }

        let mut operation = Self {
            service,
            socket: None,
            state,
        };

        // SAFETY: the reference was just initialized by a successful call.
        let socket = unsafe { sys::DNSServiceRefSockFD(service) };

        // The reactor registration needs a runtime context, which a caller
        // on the UI thread does not have.
        let _runtime = runtime().enter();

        operation.socket = Some(
            AsyncFd::with_interest(socket, Interest::READABLE)
                .context("cannot watch the mDNSResponder socket")?,
        );

        Ok(operation)
    }

    /// Wait for replies and run their callbacks.
    async fn process(&mut self) -> Result<()> {
        let Some(socket) = &self.socket else {
            bail!("the operation has stopped");
        };

        let mut ready = socket.readable().await?;

        // The reactor reports readiness on an edge, so every reply already
        // queued is processed before the readiness is cleared.
        loop {
            // SAFETY: the socket is readable, so this reads one reply
            // without blocking, and the callback's context is `self.state`,
            // which nothing else borrows during the call.
            let code = unsafe { sys::DNSServiceProcessResult(self.service) };

            if code != 0 {
                bail!("DNSServiceProcessResult failed with error {code}");
            }

            if !readable_now(*socket.get_ref()) {
                break;
            }
        }

        ready.clear_ready();

        Ok(())
    }
}

impl<S> Drop for Operation<S> {
    fn drop(&mut self) {
        self.socket.take();

        // SAFETY: the reference is valid and nothing uses it afterwards.
        unsafe { sys::DNSServiceRefDeallocate(self.service) };
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Browse for instances of `regtype`, such as `_http._tcp`, in the local
/// domain until the returned query is dropped.
pub fn browse(regtype: &str) -> Result<(Query, UnboundedReceiver<BrowseReply>)> {
    let regtype = CString::new(regtype)?;
    let (sender, receiver) = mpsc::unbounded_channel();

    let operation = Operation::start(sender, "DNSServiceBrowse", |service, context| {
        // SAFETY: the type name outlives the call, which copies it, and
        // `context` points at the sender the callback expects.
        unsafe {
            sys::DNSServiceBrowse(
                service,
                0,
                0,
                regtype.as_ptr(),
                ptr::null(),
                Some(on_browse),
                context,
            )
        }
    })?;

    Ok((run(operation), receiver))
}

unsafe extern "C" fn on_browse(
    _service: ServiceRef,
    flags: u32,
    interface: u32,
    error: i32,
    name: *const c_char,
    regtype: *const c_char,
    domain: *const c_char,
    context: *mut c_void,
) {
    if error != 0 {
        return;
    }

    // SAFETY: the context is the browse operation's sender, and the strings
    // are NUL-terminated for the duration of the callback.
    let (sender, name, regtype, domain) = unsafe {
        (
            &*context.cast::<UnboundedSender<BrowseReply>>(),
            text(name),
            text(regtype),
            text(domain),
        )
    };

    let _ = sender.send(BrowseReply {
        added: flags & FLAGS_ADD != 0,
        name,
        regtype,
        domain,
        interface,
    });
}

/// Resolve an instance a browse reported until the returned query is
/// dropped. The receiver gets the instance once and again each time its
/// SRV or TXT record changes.
pub fn resolve(
    name: &str,
    regtype: &str,
    domain: &str,
    interface: u32,
) -> Result<(Query, UnboundedReceiver<ServiceInstance>)> {
    let name = CString::new(name)?;
    let regtype = CString::new(regtype)?;
    let domain = CString::new(domain)?;
    let (sender, receiver) = mpsc::unbounded_channel();

    let operation = Operation::start(sender, "DNSServiceResolve", |service, context| {
        // SAFETY: the names outlive the call, which copies them, and
        // `context` points at the sender the callback expects.
        unsafe {
            sys::DNSServiceResolve(
                service,
                0,
                interface,
                name.as_ptr(),
                regtype.as_ptr(),
                domain.as_ptr(),
                Some(on_resolve),
                context,
            )
        }
    })?;

    Ok((run(operation), receiver))
}

unsafe extern "C" fn on_resolve(
    _service: ServiceRef,
    _flags: u32,
    _interface: u32,
    error: i32,
    _fullname: *const c_char,
    host: *const c_char,
    port: u16,
    txt_len: u16,
    txt: *const u8,
    context: *mut c_void,
) {
    if error != 0 {
        return;
    }

    // SAFETY: the context is the resolve operation's sender; the host name
    // is NUL-terminated and the TXT record holds `txt_len` bytes for the
    // duration of the callback.
    let (sender, host, txt) = unsafe {
        (
            &*context.cast::<UnboundedSender<ServiceInstance>>(),
            text(host),
            if txt.is_null() {
                &[][..]
            } else {
                slice::from_raw_parts(txt, usize::from(txt_len))
            },
        )
    };

    let _ = sender.send(ServiceInstance {
        host,
        port: u16::from_be(port),
        properties: parse_txt(txt),
    });
}

/// The IPv4 addresses of `host`, such as `office-mac.local.`, from the
/// first batch of answers. It has no timeout of its own; dropping the
/// future stops the lookup.
pub async fn ipv4_addresses(host: &str) -> Result<Vec<Ipv4Addr>> {
    let host = CString::new(host)?;

    let mut operation = Operation::start(
        AddressLookup::default(),
        "DNSServiceGetAddrInfo",
        |service, context| {
            // SAFETY: the host name outlives the call, which copies it, and
            // `context` points at the lookup the callback expects.
            unsafe {
                sys::DNSServiceGetAddrInfo(
                    service,
                    0,
                    0,
                    PROTOCOL_IPV4,
                    host.as_ptr(),
                    Some(on_address),
                    context,
                )
            }
        },
    )?;

    while !operation.state.done {
        operation.process().await?;
    }

    Ok(operation.state.addresses.clone())
}

#[derive(Default)]
struct AddressLookup {
    addresses: Vec<Ipv4Addr>,
    done: bool,
}

unsafe extern "C" fn on_address(
    _service: ServiceRef,
    flags: u32,
    _interface: u32,
    error: i32,
    _host: *const c_char,
    address: *const libc::sockaddr,
    _ttl: u32,
    context: *mut c_void,
) {
    // SAFETY: the context is the lookup this operation owns.
    let lookup = unsafe { &mut *context.cast::<AddressLookup>() };

    // SAFETY: a non-null address of family AF_INET is a `sockaddr_in`.
    let ipv4 = unsafe {
        (error == 0
            && flags & FLAGS_ADD != 0
            && !address.is_null()
            && c_int::from((*address).sa_family) == libc::AF_INET)
            .then(|| (*address.cast::<libc::sockaddr_in>()).sin_addr.s_addr)
    };

    if let Some(address) = ipv4 {
        lookup.addresses.push(Ipv4Addr::from(u32::from_be(address)));
    }

    if error != 0 || flags & FLAGS_MORE_COMING == 0 {
        lookup.done = true;
    }
}

impl Registration {
    /// Publish `record` under this computer's own host name, which the
    /// daemon answers for on every interface.
    pub fn new(record: &ServiceRecord) -> Result<Self> {
        let name = CString::new(record.name)?;
        let regtype = CString::new(record.regtype)?;
        let txt = txt_record(record.properties)?;

        let mut service = ptr::null_mut();

        // SAFETY: the strings and TXT record outlive the call, which copies
        // them. Without a callback the daemon reports nothing back, so the
        // connection needs no reader and the record can be withdrawn
        // synchronously on drop.
        let code = unsafe {
            sys::DNSServiceRegister(
                &mut service,
                0,
                0,
                name.as_ptr(),
                regtype.as_ptr(),
                ptr::null(),
                ptr::null(),
                record.port.to_be(),
                txt.len() as u16,
                txt.as_ptr().cast(),
                None,
                ptr::null_mut(),
            )
        };

        if code != 0 {
            bail!("DNSServiceRegister failed with error {code}");
        }

        Ok(Self { service })
    }

    /// Replace the record's TXT properties in place. The instance keeps its
    /// name, so a browser sees the record change instead of the instance
    /// leaving and coming back.
    pub fn set_properties(&self, properties: &[(&str, &str)]) -> Result<()> {
        let txt = txt_record(properties)?;

        // SAFETY: the reference is a live registration and a null record
        // reference names its TXT record; the call copies `txt`.
        let code = unsafe {
            sys::DNSServiceUpdateRecord(
                self.service,
                ptr::null_mut(),
                0,
                txt.len() as u16,
                txt.as_ptr().cast(),
                0,
            )
        };

        if code != 0 {
            bail!("DNSServiceUpdateRecord failed with error {code}");
        }

        Ok(())
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: the reference is valid and nothing uses it afterwards.
        // Deallocating withdraws the record and announces its removal.
        unsafe { sys::DNSServiceRefDeallocate(self.service) };
    }
}

/// Process `operation`'s replies on a runtime task until the returned query
/// is dropped.
fn run<S: Send + 'static>(mut operation: Operation<S>) -> Query {
    let task = runtime().spawn(async move {
        loop {
            if let Err(error) = operation.process().await {
                warn!(%error, "a DNS-SD operation stopped");

                break;
            }
        }
    });

    Query {
        task: task.abort_handle(),
    }
}

/// Encode TXT properties as `key=value` strings, each behind a length byte.
fn txt_record(properties: &[(&str, &str)]) -> Result<Vec<u8>> {
    let mut txt = Vec::new();

    for (key, value) in properties {
        let entry = format!("{key}={value}");
        let len = u8::try_from(entry.len()).context("a TXT entry is longer than 255 bytes")?;

        txt.push(len);
        txt.extend_from_slice(entry.as_bytes());
    }

    if u16::try_from(txt.len()).is_err() {
        bail!("the TXT record is longer than 65535 bytes");
    }

    Ok(txt)
}

/// Decode a TXT record into key and value pairs. An entry without `=` is a
/// key with an empty value.
fn parse_txt(mut txt: &[u8]) -> Vec<(String, String)> {
    let mut properties = Vec::new();

    while let Some((&len, rest)) = txt.split_first() {
        let len = usize::from(len).min(rest.len());
        let (entry, rest) = rest.split_at(len);
        let entry = String::from_utf8_lossy(entry);

        if let Some((key, value)) = entry.split_once('=') {
            properties.push((key.to_owned(), value.to_owned()));
        } else if !entry.is_empty() {
            properties.push((entry.into_owned(), String::new()));
        }

        txt = rest;
    }

    properties
}

/// # Safety
///
/// `value` must be null or point at a NUL-terminated string.
unsafe fn text(value: *const c_char) -> String {
    if value.is_null() {
        return String::new();
    }

    // SAFETY: the caller passes a NUL-terminated string.
    unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned()
}

/// Whether `socket` has data to read now.
fn readable_now(socket: RawFd) -> bool {
    let mut poll = libc::pollfd {
        fd: socket,
        events: libc::POLLIN,
        revents: 0,
    };

    // SAFETY: one valid pollfd and a zero timeout, so the call never waits.
    unsafe { libc::poll(&mut poll, 1, 0) > 0 && poll.revents & libc::POLLIN != 0 }
}
