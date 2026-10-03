//! DNS-SD through the Windows DNS Client service. The service sends and
//! answers the multicast queries, so the calling process never binds UDP
//! 5353 and the firewall has no listening socket to prompt about.
//!
//! The service calls back on its own thread pool threads with an opaque
//! context pointer. Each request stores its route in a table keyed by a
//! number and passes that number as the context, so a callback that arrives
//! after its request was dropped finds no route and only frees its data.
//!
//! `DnsServiceBrowse` reports instances as they answer but never reports one
//! leaving; a caller that needs removals has to browse again and compare.

use std::collections::HashMap;
use std::ffi::c_void;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, OnceLock};
use std::{mem, ptr, slice};

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tracing::warn;
use windows_sys::Win32::Foundation::{DNS_REQUEST_PENDING, ERROR_SUCCESS};
use windows_sys::Win32::NetworkManagement::Dns::{
    DNS_QUERY_REQUEST_VERSION1, DNS_RECORDW, DNS_SERVICE_BROWSE_REQUEST,
    DNS_SERVICE_BROWSE_REQUEST_0, DNS_SERVICE_CANCEL, DNS_SERVICE_INSTANCE,
    DNS_SERVICE_REGISTER_REQUEST, DNS_SERVICE_RESOLVE_REQUEST, DNS_TYPE_PTR, DnsFree,
    DnsFreeRecordList, IP6_ADDRESS,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};
use windows_sys::core::{PCWSTR, PWSTR, s, w};

use crate::windows::win32_string;

/// What `GetProcAddress` returns, before it is cast to the real signature.
type LoadedFn = unsafe extern "system" fn() -> isize;

type BrowseFn =
    unsafe extern "system" fn(*const DNS_SERVICE_BROWSE_REQUEST, *mut DNS_SERVICE_CANCEL) -> i32;

type ResolveFn =
    unsafe extern "system" fn(*const DNS_SERVICE_RESOLVE_REQUEST, *mut DNS_SERVICE_CANCEL) -> i32;

type CancelFn = unsafe extern "system" fn(*const DNS_SERVICE_CANCEL) -> i32;

type RegisterFn =
    unsafe extern "system" fn(*const DNS_SERVICE_REGISTER_REQUEST, *mut DNS_SERVICE_CANCEL) -> u32;

type ConstructInstanceFn = unsafe extern "system" fn(
    PCWSTR,
    PCWSTR,
    *const u32,
    *const IP6_ADDRESS,
    u16,
    u16,
    u16,
    u32,
    *const PCWSTR,
    *const PCWSTR,
) -> *mut DNS_SERVICE_INSTANCE;

type FreeInstanceFn = unsafe extern "system" fn(*const DNS_SERVICE_INSTANCE);

/// The DNS-SD functions of dnsapi.dll. They are looked up at run time:
/// linking them makes the loader refuse to start the whole application on a
/// Windows build that lacks them, where only discovery should be missing.
struct Api {
    browse: BrowseFn,
    browse_cancel: CancelFn,
    resolve: ResolveFn,
    resolve_cancel: CancelFn,
    register: RegisterFn,
    deregister: RegisterFn,
    construct_instance: ConstructInstanceFn,
    free_instance: FreeInstanceFn,
}

/// Routes for browse callbacks: the instance names each browse hears.
static BROWSES: LazyLock<Mutex<HashMap<usize, UnboundedSender<String>>>> =
    LazyLock::new(Default::default);

/// Routes for resolve callbacks. A resolve completes once, so its route is
/// removed by whichever comes first: the callback or the request's drop.
static RESOLVES: LazyLock<Mutex<HashMap<usize, oneshot::Sender<Option<ServiceInstance>>>>> =
    LazyLock::new(Default::default);

/// Registrations by context number, held until the service has made every
/// callback it owes for them.
static REGISTRATIONS: LazyLock<Mutex<HashMap<usize, RegistrationEntry>>> =
    LazyLock::new(Default::default);

static NEXT_CONTEXT: AtomicUsize = AtomicUsize::new(1);

/// A running `DnsServiceBrowse`. Dropping it cancels the browse.
pub struct Browse {
    api: &'static Api,
    context: usize,
    cancel: Box<DNS_SERVICE_CANCEL>,
    _request: Box<DNS_SERVICE_BROWSE_REQUEST>,
    _query: Vec<u16>,
}

/// A resolved service instance.
pub struct ServiceInstance {
    /// The TXT record as key and value pairs.
    pub properties: Vec<(String, String)>,

    pub ipv4: Option<Ipv4Addr>,
    pub port: u16,
}

/// A service to publish.
pub struct ServiceRecord<'a> {
    /// The full instance name: instance label, service type, and domain.
    pub name: &'a str,

    /// The host whose addresses the record points at, such as
    /// [`local_host_name`].
    pub host: &'a str,

    pub port: u16,
    pub properties: &'a [(&'a str, &'a str)],
}

/// A published service. Dropping it withdraws the record.
pub struct Registration {
    context: usize,
}

/// One `DnsServiceResolve` call, cancelled on drop unless it completed.
struct ResolveRequest {
    api: &'static Api,
    context: usize,
    cancel: Box<DNS_SERVICE_CANCEL>,
    _request: Box<DNS_SERVICE_RESOLVE_REQUEST>,
    _name: Vec<u16>,
}

/// A request handed to `DnsServiceRegister`. The service reads the request
/// and its instance until it completes, so both stay allocated until it has
/// called back for the registration and, once withdrawn, for the
/// deregistration.
struct RegistrationEntry {
    api: &'static Api,
    request: Box<DNS_SERVICE_REGISTER_REQUEST>,
    callbacks_owed: u8,
    withdrawn: bool,
}

// SAFETY: the cancel handle and request buffers are owned allocations the
// DNS Client service reads by address; it accepts cancellation from any
// thread, and nothing here reads them through shared references.
unsafe impl Send for Browse {}

// SAFETY: as for `Browse`.
unsafe impl Send for ResolveRequest {}

// SAFETY: the request and its instance are owned allocations that only the
// DNS Client service and the entry's final drop read, under the table lock.
unsafe impl Send for RegistrationEntry {}

impl Api {
    fn get() -> Result<&'static Self> {
        static API: OnceLock<Option<Api>> = OnceLock::new();

        API.get_or_init(Self::load)
            .as_ref()
            .ok_or_else(|| anyhow!("this Windows version has no DNS-SD service"))
    }

    fn load() -> Option<Self> {
        // SAFETY: dnsapi.dll is a system library loaded from System32 only,
        // and it stays loaded for the life of the process. Each symbol is
        // transmuted to the signature windns.h declares for it.
        unsafe {
            let module = LoadLibraryExW(
                w!("dnsapi.dll"),
                ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            );

            if module.is_null() {
                return None;
            }

            Some(Self {
                browse: mem::transmute::<LoadedFn, BrowseFn>(GetProcAddress(
                    module,
                    s!("DnsServiceBrowse"),
                )?),
                browse_cancel: mem::transmute::<LoadedFn, CancelFn>(GetProcAddress(
                    module,
                    s!("DnsServiceBrowseCancel"),
                )?),
                resolve: mem::transmute::<LoadedFn, ResolveFn>(GetProcAddress(
                    module,
                    s!("DnsServiceResolve"),
                )?),
                resolve_cancel: mem::transmute::<LoadedFn, CancelFn>(GetProcAddress(
                    module,
                    s!("DnsServiceResolveCancel"),
                )?),
                register: mem::transmute::<LoadedFn, RegisterFn>(GetProcAddress(
                    module,
                    s!("DnsServiceRegister"),
                )?),
                deregister: mem::transmute::<LoadedFn, RegisterFn>(GetProcAddress(
                    module,
                    s!("DnsServiceDeRegister"),
                )?),
                construct_instance: mem::transmute::<LoadedFn, ConstructInstanceFn>(
                    GetProcAddress(module, s!("DnsServiceConstructInstance"))?,
                ),
                free_instance: mem::transmute::<LoadedFn, FreeInstanceFn>(GetProcAddress(
                    module,
                    s!("DnsServiceFreeInstance"),
                )?),
            })
        }
    }
}

impl Browse {
    /// Browse for instances of `service_type`, such as `_http._tcp.local`.
    /// The receiver gets each instance's full name every time it answers,
    /// and never hears of one leaving.
    pub fn start(service_type: &str) -> Result<(Self, UnboundedReceiver<String>)> {
        let api = Api::get()?;
        let context = next_context();
        let query = win32_string(service_type);
        let (sender, receiver) = mpsc::unbounded_channel();

        let request = Box::new(DNS_SERVICE_BROWSE_REQUEST {
            Version: DNS_QUERY_REQUEST_VERSION1,
            InterfaceIndex: 0,
            QueryName: query.as_ptr(),
            Anonymous: DNS_SERVICE_BROWSE_REQUEST_0 {
                pBrowseCallback: Some(on_browse),
            },
            pQueryContext: context as *mut c_void,
        });

        let mut cancel = Box::new(DNS_SERVICE_CANCEL::default());

        BROWSES.lock().insert(context, sender);

        // SAFETY: the request, its query name, and the cancel handle stay
        // allocated in the returned value until the browse is cancelled.
        let status = unsafe { (api.browse)(&*request, &mut *cancel) };

        if status != DNS_REQUEST_PENDING {
            BROWSES.lock().remove(&context);

            bail!("DnsServiceBrowse failed with status {status}");
        }

        let browse = Self {
            api,
            context,
            cancel,
            _request: request,
            _query: query,
        };

        Ok((browse, receiver))
    }
}

impl Drop for Browse {
    fn drop(&mut self) {
        // SAFETY: the handle belongs to a browse that is still running; a
        // browse never completes by itself.
        unsafe { (self.api.browse_cancel)(&*self.cancel) };

        BROWSES.lock().remove(&self.context);
    }
}

/// Sends the instance names a browse answer lists, and frees the answer.
unsafe extern "system" fn on_browse(
    status: u32,
    context: *const c_void,
    records: *const DNS_RECORDW,
) {
    let mut names = Vec::new();
    let mut record = records;

    while status == ERROR_SUCCESS && !record.is_null() {
        // SAFETY: the service hands over a valid record list that stays
        // allocated until freed below.
        let current = unsafe { &*record };

        // A zero TTL announces that the instance is leaving.
        if current.wType == DNS_TYPE_PTR && current.dwTtl > 0 {
            // SAFETY: a PTR record's data is its PTR member.
            names.extend(unsafe { from_wide(current.Data.PTR.pNameHost) });
        }

        record = current.pNext;
    }

    if !records.is_null() {
        // SAFETY: the list came from the service, which leaves freeing it to
        // the callback.
        unsafe { DnsFree(records.cast(), DnsFreeRecordList) };
    }

    if let Some(sender) = BROWSES.lock().get(&(context as usize)) {
        for name in names {
            let _ = sender.send(name);
        }
    }
}

/// Resolve the instance with full name `name`, or `None` if the service
/// cannot. Dropping the future cancels the resolve; it has no timeout of
/// its own.
pub async fn resolve(name: &str) -> Option<ServiceInstance> {
    let (sender, receiver) = oneshot::channel();

    let _request = ResolveRequest::start(name, sender)
        .inspect_err(|error| warn!(%error, "cannot resolve a DNS-SD instance"))
        .ok()?;

    receiver.await.ok()?
}

impl ResolveRequest {
    fn start(name: &str, reply: oneshot::Sender<Option<ServiceInstance>>) -> Result<Self> {
        let api = Api::get()?;
        let context = next_context();

        let mut name = win32_string(name);

        let request = Box::new(DNS_SERVICE_RESOLVE_REQUEST {
            Version: DNS_QUERY_REQUEST_VERSION1,
            InterfaceIndex: 0,
            QueryName: name.as_mut_ptr(),
            pResolveCompletionCallback: Some(on_resolved),
            pQueryContext: context as *mut c_void,
        });

        let mut cancel = Box::new(DNS_SERVICE_CANCEL::default());

        RESOLVES.lock().insert(context, reply);

        // SAFETY: the request, its name, and the cancel handle stay
        // allocated in the returned value until the resolve completes or is
        // cancelled.
        let status = unsafe { (api.resolve)(&*request, &mut *cancel) };

        if status != DNS_REQUEST_PENDING {
            RESOLVES.lock().remove(&context);

            bail!("DnsServiceResolve failed with status {status}");
        }

        Ok(Self {
            api,
            context,
            cancel,
            _request: request,
            _name: name,
        })
    }
}

impl Drop for ResolveRequest {
    fn drop(&mut self) {
        // A route still in the table means the service has not called back,
        // so the resolve is still running and needs cancelling.
        if RESOLVES.lock().remove(&self.context).is_some() {
            // SAFETY: the handle belongs to a resolve that has not
            // completed.
            unsafe { (self.api.resolve_cancel)(&*self.cancel) };
        }
    }
}

/// Copies out the resolved instance, frees it, and sends the copy.
unsafe extern "system" fn on_resolved(
    status: u32,
    context: *const c_void,
    instance: *const DNS_SERVICE_INSTANCE,
) {
    let resolved = if status == ERROR_SUCCESS && !instance.is_null() {
        // SAFETY: the service hands over a valid instance that stays
        // allocated until freed below.
        Some(unsafe { copy_instance(&*instance) })
    } else {
        None
    };

    free_instance(instance);

    if let Some(reply) = RESOLVES.lock().remove(&(context as usize)) {
        let _ = reply.send(resolved);
    }
}

/// # Safety
///
/// `instance` must be a valid instance from the DNS Client service.
unsafe fn copy_instance(instance: &DNS_SERVICE_INSTANCE) -> ServiceInstance {
    let count = instance.dwPropertyCount as usize;

    let mut properties = Vec::with_capacity(count);

    for index in 0..count {
        // SAFETY: `keys` and `values` each hold `dwPropertyCount` strings.
        let (key, value) = unsafe {
            (
                from_wide(*instance.keys.add(index)),
                from_wide(*instance.values.add(index)),
            )
        };

        if let Some(key) = key {
            properties.push((key, value.unwrap_or_default()));
        }
    }

    // IP4_ADDRESS holds the address in network byte order, so its bytes in
    // memory are the octets in order.
    let ipv4 = (!instance.ip4Address.is_null())
        // SAFETY: a non-null address points at one IP4_ADDRESS.
        .then(|| Ipv4Addr::from(unsafe { *instance.ip4Address }.to_ne_bytes()));

    ServiceInstance {
        properties,
        ipv4,
        port: instance.wPort,
    }
}

impl Registration {
    /// Ask the service to publish `record`. The service completes the
    /// registration in the background and logs a failure it reports later.
    pub fn new(record: &ServiceRecord) -> Result<Self> {
        let api = Api::get()?;
        let name = win32_string(record.name);
        let host = win32_string(record.host);

        let keys: Vec<Vec<u16>> = record
            .properties
            .iter()
            .map(|(key, _)| win32_string(key))
            .collect();

        let values: Vec<Vec<u16>> = record
            .properties
            .iter()
            .map(|(_, value)| win32_string(value))
            .collect();

        let key_pointers: Vec<PCWSTR> = keys.iter().map(|key| key.as_ptr()).collect();
        let value_pointers: Vec<PCWSTR> = values.iter().map(|value| value.as_ptr()).collect();

        // SAFETY: every string is NUL-terminated and outlives the call, which
        // copies them into the instance it allocates. No address is passed,
        // so the service answers with the addresses of `host`.
        let instance = unsafe {
            (api.construct_instance)(
                name.as_ptr(),
                host.as_ptr(),
                ptr::null(),
                ptr::null(),
                record.port,
                0,
                0,
                key_pointers.len() as u32,
                key_pointers.as_ptr(),
                value_pointers.as_ptr(),
            )
        };

        if instance.is_null() {
            bail!("cannot build the DNS-SD record");
        }

        let context = next_context();

        let request = Box::new(DNS_SERVICE_REGISTER_REQUEST {
            Version: DNS_QUERY_REQUEST_VERSION1,
            InterfaceIndex: 0,
            pServiceInstance: instance,
            pRegisterCompletionCallback: Some(on_registered),
            pQueryContext: context as *mut c_void,
            hCredentials: ptr::null_mut(),
            unicastEnabled: 0,
        });

        // The box keeps its address when the entry moves into the table.
        let request_address: *const DNS_SERVICE_REGISTER_REQUEST = &*request;

        // The entry goes into the table before the call, which may call
        // back on any thread, and the lock is not held across it: a
        // callback on this thread would otherwise wait on it forever.
        REGISTRATIONS.lock().insert(
            context,
            RegistrationEntry {
                api,
                request,
                callbacks_owed: 1,
                withdrawn: false,
            },
        );

        // SAFETY: the request and its instance stay allocated in the table
        // until the service has called back for them.
        let status = unsafe { (api.register)(request_address, ptr::null_mut()) };

        if status != DNS_REQUEST_PENDING as u32 {
            REGISTRATIONS.lock().remove(&context);

            bail!("DnsServiceRegister failed with status {status}");
        }

        Ok(Self { context })
    }
}

impl Drop for Registration {
    /// Withdraw the record. The entry is freed once the service has called
    /// back for everything it started.
    fn drop(&mut self) {
        let context = self.context;

        let (api, request_address): (_, *const DNS_SERVICE_REGISTER_REQUEST) = {
            let mut registrations = REGISTRATIONS.lock();

            let Some(entry) = registrations.get_mut(&context) else {
                return;
            };

            // Counted before the call so a callback that arrives first
            // cannot free the entry while the call still reads it.
            entry.withdrawn = true;
            entry.callbacks_owed += 1;

            (entry.api, &*entry.request)
        };

        // SAFETY: the request is the one the service registered; the owed
        // callback keeps it allocated in the table.
        let status = unsafe { (api.deregister)(request_address, ptr::null_mut()) };

        if status == DNS_REQUEST_PENDING as u32 {
            return;
        }

        warn!(status, "DnsServiceDeRegister failed");

        let mut registrations = REGISTRATIONS.lock();

        if let Some(entry) = registrations.get_mut(&context) {
            entry.callbacks_owed -= 1;

            if entry.callbacks_owed == 0 {
                registrations.remove(&context);
            }
        }
    }
}

impl Drop for RegistrationEntry {
    fn drop(&mut self) {
        // SAFETY: the instance came from DnsServiceConstructInstance and the
        // service owes no more callbacks that read it.
        unsafe { (self.api.free_instance)(self.request.pServiceInstance) };
    }
}

/// Called once when a registration completes and once when a
/// deregistration does, with a copy of the instance to free.
unsafe extern "system" fn on_registered(
    status: u32,
    context: *const c_void,
    instance: *const DNS_SERVICE_INSTANCE,
) {
    let mut registrations = REGISTRATIONS.lock();

    if let Some(entry) = registrations.get_mut(&(context as usize)) {
        entry.callbacks_owed = entry.callbacks_owed.saturating_sub(1);

        if status != ERROR_SUCCESS && !entry.withdrawn {
            warn!(status, "the DNS-SD record could not be published");
        }

        if entry.withdrawn && entry.callbacks_owed == 0 {
            registrations.remove(&(context as usize));
        }
    }

    drop(registrations);

    free_instance(instance);
}

/// This computer's mDNS name, `<computer name>.local`. The DNS Client
/// service answers address queries for it on every interface.
pub fn local_host_name() -> Result<String> {
    let mut buffer = [0u16; 256];
    let mut size = buffer.len() as u32;

    // SAFETY: `size` holds the buffer's length in characters.
    let ok = unsafe { GetComputerNameExW(ComputerNameDnsHostname, buffer.as_mut_ptr(), &mut size) };

    if ok == 0 {
        bail!("cannot read this computer's host name");
    }

    Ok(format!(
        "{}.local",
        String::from_utf16_lossy(&buffer[..size as usize])
    ))
}

/// Free an instance the service handed to a callback.
fn free_instance(instance: *const DNS_SERVICE_INSTANCE) {
    if instance.is_null() {
        return;
    }

    // A callback only runs for a request that `Api::get` loaded.
    if let Ok(api) = Api::get() {
        // SAFETY: the service leaves freeing callback instances to the
        // callee, and nothing reads it afterwards.
        unsafe { (api.free_instance)(instance) };
    }
}

fn next_context() -> usize {
    NEXT_CONTEXT.fetch_add(1, Ordering::Relaxed)
}

/// # Safety
///
/// `text` must be null or point at a NUL-terminated UTF-16 string.
unsafe fn from_wide(text: PWSTR) -> Option<String> {
    if text.is_null() {
        return None;
    }

    let mut len = 0;

    // SAFETY: the string is NUL-terminated.
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }

    // SAFETY: `len` characters precede the terminator.
    Some(String::from_utf16_lossy(unsafe {
        slice::from_raw_parts(text, len)
    }))
}
