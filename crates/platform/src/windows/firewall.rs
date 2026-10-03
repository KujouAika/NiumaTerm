//! Windows Firewall rules that let other computers reach this executable.
//!
//! Remote sessions accept inbound connections in two places: the LAN
//! listener (TCP) and the waiting side of a direct path (UDP). Windows asks
//! about inbound access only when a program first listens, so a computer
//! that never hosted has no rule, and one whose user once cancelled that
//! prompt has a block rule that wins over any allow rule. Reading the
//! rules needs no privileges; changing them runs this executable again
//! elevated with [`CONFIGURE_FIREWALL_FLAG`].

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;
use std::{io, iter, mem};

use anyhow::{Result, anyhow, bail};
use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, VARIANT_FALSE, VARIANT_TRUE};
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRule, NET_FW_ACTION_ALLOW, NET_FW_ACTION_BLOCK, NET_FW_IP_PROTOCOL_ANY,
    NET_FW_IP_PROTOCOL_TCP, NET_FW_IP_PROTOCOL_UDP, NET_FW_MODIFY_STATE_GP_OVERRIDE,
    NET_FW_MODIFY_STATE_INBOUND_BLOCKED, NET_FW_PROFILE_TYPE2, NET_FW_PROFILE2_ALL,
    NET_FW_RULE_DIR_IN, NetFwPolicy2, NetFwRule,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    IDispatch,
};
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, Interface as _};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};

/// The argument that makes this executable configure the firewall for
/// itself and exit, which the elevated copy started by [`request_allow`]
/// receives.
pub const CONFIGURE_FIREWALL_FLAG: &str = "--configure-firewall";

/// The name of the allow rule this module adds. Windows names the rules its
/// own prompt adds after the file, which several builds share, so rules are
/// matched by program path and this name only finds the one added here.
const RULE_NAME: &str = "NiumaTerm remote sessions";

const RULE_GROUP: &str = "NiumaTerm";

const RULE_DESCRIPTION: &str = "Lets paired devices reach NiumaTerm on the local network and over direct connections through NATs.";

/// What the firewall does with inbound connections to a program on the
/// network profiles in use now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirewallStatus {
    /// The firewall is off on every profile in use.
    Off,
    /// Inbound TCP and UDP from any address reach the program.
    Allowed,
    /// No rule admits the program, so inbound connections are dropped.
    Missing,
    /// A block rule, or a profile that blocks every inbound connection,
    /// stops the program whatever the allow rules say.
    Blocked,
    /// Group policy overrides local rules, so only an administrator of the
    /// policy can change the outcome.
    Managed,
}

/// The outcome of asking to change the firewall elevated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElevationOutcome {
    Configured,
    /// The user declined the elevation prompt.
    Declined,
}

/// Read what the firewall does with inbound connections to `program`. The
/// call blocks while the firewall service enumerates its rules.
pub fn status(program: &Path) -> Result<FirewallStatus> {
    let _com = ComScope::enter()?;

    unsafe {
        let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)?;

        let current = policy.CurrentProfileTypes()?;

        let mut enabled = 0;
        let mut block_all = false;

        for bit in [1, 2, 4] {
            let profile = NET_FW_PROFILE_TYPE2(bit);

            if current & bit != 0 && policy.get_FirewallEnabled(profile)? == VARIANT_TRUE {
                enabled |= bit;
                block_all |= policy.get_BlockAllInboundTraffic(profile)? == VARIANT_TRUE;
            }
        }

        if enabled == 0 {
            return Ok(FirewallStatus::Off);
        }

        match policy.LocalPolicyModifyState()? {
            NET_FW_MODIFY_STATE_GP_OVERRIDE => return Ok(FirewallStatus::Managed),
            NET_FW_MODIFY_STATE_INBOUND_BLOCKED => return Ok(FirewallStatus::Blocked),
            _ => {}
        }

        if block_all {
            return Ok(FirewallStatus::Blocked);
        }

        let mut tcp = false;
        let mut udp = false;

        for rule in program_rules(&policy, program)? {
            if rule.Enabled()? != VARIANT_TRUE || rule.Profiles()? & enabled == 0 {
                continue;
            }

            let protocol = rule.Protocol()?;
            let covers = |wanted: i32| protocol == NET_FW_IP_PROTOCOL_ANY.0 || protocol == wanted;

            match rule.Action()? {
                NET_FW_ACTION_BLOCK => return Ok(FirewallStatus::Blocked),
                NET_FW_ACTION_ALLOW if unrestricted(&rule)? => {
                    tcp |= covers(NET_FW_IP_PROTOCOL_TCP.0);
                    udp |= covers(NET_FW_IP_PROTOCOL_UDP.0);
                }
                _ => {}
            }
        }

        Ok(if tcp && udp {
            FirewallStatus::Allowed
        } else {
            FirewallStatus::Missing
        })
    }
}

/// Let inbound TCP and UDP reach `program` on every profile: disable the
/// block rules for it and add, or enable again, this module's allow rule.
/// Needs administrator rights.
pub fn allow(program: &Path) -> Result<()> {
    let _com = ComScope::enter()?;

    unsafe {
        let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)?;

        let mut ours = None;

        for rule in program_rules(&policy, program)? {
            // Disabled, not removed: rules are removed by name, and
            // the names Windows gives are shared by other builds' rules.
            if rule.Action()? == NET_FW_ACTION_BLOCK {
                rule.SetEnabled(VARIANT_FALSE)?;
            } else if rule.Name()? == RULE_NAME {
                ours = Some(rule);
            }
        }

        let rule = match ours {
            Some(rule) => rule,
            None => {
                let rule: INetFwRule = CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER)?;

                rule.SetName(&BSTR::from(RULE_NAME))?;
                rule.SetDescription(&BSTR::from(RULE_DESCRIPTION))?;
                rule.SetGrouping(&BSTR::from(RULE_GROUP))?;

                rule.SetApplicationName(&BSTR::from(
                    program.as_os_str().to_string_lossy().as_ref(),
                ))?;

                rule.SetDirection(NET_FW_RULE_DIR_IN)?;
                rule.SetAction(NET_FW_ACTION_ALLOW)?;

                policy.Rules()?.Add(&rule)?;

                rule
            }
        };

        rule.SetProtocol(NET_FW_IP_PROTOCOL_ANY.0)?;
        rule.SetProfiles(NET_FW_PROFILE2_ALL.0)?;
        rule.SetEnabled(VARIANT_TRUE)?;
    }

    Ok(())
}

/// Run `program` elevated with [`CONFIGURE_FIREWALL_FLAG`] and wait for it.
/// Windows shows the elevation prompt; declining it is not an error.
pub fn request_allow(program: &Path) -> Result<ElevationOutcome> {
    let verb = wide("runas");
    let file = wide(program.as_os_str());
    let parameters = wide(CONFIGURE_FIREWALL_FLAG);

    let mut info: SHELLEXECUTEINFOW = unsafe { mem::zeroed() };

    info.cbSize = u32::try_from(mem::size_of::<SHELLEXECUTEINFOW>())?;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();

    unsafe {
        if ShellExecuteExW(&mut info) == 0 {
            if GetLastError() == ERROR_CANCELLED {
                return Ok(ElevationOutcome::Declined);
            }

            bail!(
                "cannot start the elevated helper: {}",
                io::Error::last_os_error()
            );
        }

        if info.hProcess.is_null() {
            bail!("the elevated helper started without a process handle");
        }

        WaitForSingleObject(info.hProcess, INFINITE);

        let mut code = 1;

        let read = GetExitCodeProcess(info.hProcess, &mut code);

        CloseHandle(info.hProcess);

        if read == 0 || code != 0 {
            bail!("the elevated helper could not change the firewall");
        }
    }

    Ok(ElevationOutcome::Configured)
}

/// Inbound rules whose program is `program`. Windows stores the path in
/// whatever case the creator gave, its own prompt in lower case.
fn program_rules(policy: &INetFwPolicy2, program: &Path) -> Result<Vec<INetFwRule>> {
    let wanted = program.as_os_str().to_string_lossy().to_lowercase();

    let rules = unsafe { policy.Rules()?._NewEnum()?.cast::<IEnumVARIANT>()? };

    let mut found = Vec::new();

    loop {
        let mut item = [VARIANT::default()];
        let mut fetched = 0;

        unsafe { rules.Next(&mut item, &mut fetched) }.ok()?;

        if fetched == 0 {
            break;
        }

        let Ok(rule) = IDispatch::try_from(&item[0]).and_then(|item| item.cast::<INetFwRule>())
        else {
            continue;
        };

        let matches = unsafe {
            rule.Direction()? == NET_FW_RULE_DIR_IN
                && rule
                    .ApplicationName()
                    .is_ok_and(|name| name.to_string().to_lowercase() == wanted)
        };

        if matches {
            found.push(rule);
        }
    }

    Ok(found)
}

/// Whether an allow rule admits any local port from any remote address, as
/// the rules Windows' own prompt adds do.
fn unrestricted(rule: &INetFwRule) -> Result<bool> {
    let any = |value: BSTR| value.is_empty() || value == "*";

    unsafe { Ok(any(rule.LocalPorts()?) && any(rule.RemoteAddresses()?)) }
}

fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain(iter::once(0)).collect()
}

/// COM on the calling thread for the duration of a firewall call. A thread
/// that already runs COM in another apartment, as the UI thread does, is
/// used as it is and left initialized.
struct ComScope(bool);

impl ComScope {
    fn enter() -> Result<Self> {
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

        if result == RPC_E_CHANGED_MODE {
            return Ok(Self(false));
        }

        result
            .ok()
            .map_err(|error| anyhow!("COM is unavailable: {error}"))?;

        Ok(Self(true))
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}
