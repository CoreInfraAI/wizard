//! Current-user LAN `WinINet` settings; never `WinHTTP` or RAS/VPN profiles.
#![expect(unsafe_code, reason = "WinINet uses unions and caller-owned buffers")]

use crate::{Backend, ServiceStatus, windows_config::Config};
use anyhow::{Result, ensure};
use core::{mem::size_of, ptr};
use windows::{
    Win32::{
        Foundation::{GlobalFree, HGLOBAL},
        Networking::WinInet::{
            INTERNET_OPTION_PER_CONNECTION_OPTION, INTERNET_OPTION_PROXY_SETTINGS_CHANGED,
            INTERNET_OPTION_REFRESH, INTERNET_PER_CONN, INTERNET_PER_CONN_AUTOCONFIG_URL,
            INTERNET_PER_CONN_FLAGS, INTERNET_PER_CONN_FLAGS_UI, INTERNET_PER_CONN_OPTION_LISTW,
            INTERNET_PER_CONN_OPTIONW, INTERNET_PER_CONN_OPTIONW_0, INTERNET_PER_CONN_PROXY_BYPASS,
            INTERNET_PER_CONN_PROXY_SERVER, InternetQueryOptionW, InternetSetOptionW,
        },
    },
    core::PWSTR,
};

pub(super) struct Native;

fn native_result(result: windows::core::Result<()>) -> Result<()> {
    result.map_err(|error| anyhow::anyhow!("WinINet operation failed (HRESULT {})", error.code().0))
}

fn option(kind: INTERNET_PER_CONN) -> INTERNET_PER_CONN_OPTIONW {
    INTERNET_PER_CONN_OPTIONW {
        dwOption: kind,
        Value: INTERNET_PER_CONN_OPTIONW_0::default(),
    }
}

fn list(options: &mut [INTERNET_PER_CONN_OPTIONW]) -> Result<INTERNET_PER_CONN_OPTION_LISTW> {
    Ok(INTERNET_PER_CONN_OPTION_LISTW {
        dwSize: u32::try_from(size_of::<INTERNET_PER_CONN_OPTION_LISTW>())?,
        pszConnection: PWSTR::null(),
        dwOptionCount: u32::try_from(options.len())?,
        dwOptionError: 0,
        pOptions: options.as_mut_ptr(),
    })
}

// Query allocates the three strings with GlobalAlloc, including when a later
// conversion fails. Never log their contents (PAC URLs may contain credentials).
struct Query([INTERNET_PER_CONN_OPTIONW; 4]);
impl Drop for Query {
    fn drop(&mut self) {
        for option in &self.0[1..] {
            // These entries are string options, zero-initialized before querying.
            unsafe {
                let pointer = option.Value.pszValue.0;
                if !pointer.is_null() {
                    let _ = GlobalFree(Some(HGLOBAL(pointer.cast())));
                }
            }
        }
    }
}

fn string(option: &INTERNET_PER_CONN_OPTIONW) -> Result<String> {
    // Only called on successfully queried string options. WinINet provides a
    // NUL-terminated UTF-16 allocation owned by Query until conversion completes.
    unsafe {
        let value = option.Value.pszValue;
        if value.is_null() {
            return Ok(String::new());
        }
        value
            .to_string()
            .map_err(|_| anyhow::anyhow!("invalid UTF-16 system proxy setting"))
    }
}

impl Backend for Native {
    type Snapshot = Config;

    fn read() -> Result<Config> {
        let mut query = Query([
            option(INTERNET_PER_CONN(INTERNET_PER_CONN_FLAGS_UI)),
            option(INTERNET_PER_CONN_PROXY_SERVER),
            option(INTERNET_PER_CONN_PROXY_BYPASS),
            option(INTERNET_PER_CONN_AUTOCONFIG_URL),
        ]);
        let mut list = list(&mut query.0)?;
        let mut length = list.dwSize;
        // Null session/connection selects current-user LAN settings. FLAGS_UI
        // preserves the user's WPAD choice rather than its last detection result.
        native_result(unsafe {
            InternetQueryOptionW(
                None,
                INTERNET_OPTION_PER_CONNECTION_OPTION,
                Some(ptr::from_mut(&mut list).cast()),
                &raw mut length,
            )
        })?;
        ensure!(
            list.dwOptionError == 0,
            "WinINet could not read a proxy option"
        );
        Ok(Config {
            // The first option is the DWORD flags, not an allocated string.
            flags: unsafe { query.0[0].Value.dwValue },
            server: string(&query.0[1])?,
            bypass: string(&query.0[2])?,
            pac: string(&query.0[3])?,
        })
    }

    fn services(snapshot: &Config) -> Result<Vec<ServiceStatus>> {
        Ok(vec![snapshot.status()?])
    }

    fn prepare(snapshot: &Config, enabled: bool) -> Result<Config> {
        snapshot.prepare(enabled)
    }

    fn write(before: &Config, after: &Config) -> Result<()> {
        // WinINet has no compare-and-swap API; this narrows but cannot eliminate
        // races with other processes. Never write bypass or the retained PAC URL.
        ensure!(
            Self::read()? == *before,
            "system proxy settings changed; try again"
        );
        let mut server: Vec<u16> = after.server.encode_utf16().chain([0]).collect();
        let mut options = [
            INTERNET_PER_CONN_OPTIONW {
                dwOption: INTERNET_PER_CONN_FLAGS,
                Value: INTERNET_PER_CONN_OPTIONW_0 {
                    dwValue: after.flags,
                },
            },
            INTERNET_PER_CONN_OPTIONW {
                dwOption: INTERNET_PER_CONN_PROXY_SERVER,
                Value: INTERNET_PER_CONN_OPTIONW_0 {
                    pszValue: PWSTR(server.as_mut_ptr()),
                },
            },
        ];
        let mut list = list(&mut options)?;
        // list, options and server remain alive through the synchronous call.
        native_result(unsafe {
            InternetSetOptionW(
                None,
                INTERNET_OPTION_PER_CONNECTION_OPTION,
                Some(ptr::from_mut(&mut list).cast()),
                list.dwSize,
            )
        })?;
        ensure!(
            list.dwOptionError == 0,
            "WinINet could not apply a proxy option"
        );
        // Attempt both notifications even if the first fails; do not roll back
        // successful writes or report them as an unchanged configuration.
        let changed = native_result(unsafe {
            InternetSetOptionW(None, INTERNET_OPTION_PROXY_SETTINGS_CHANGED, None, 0)
        });
        let refresh =
            native_result(unsafe { InternetSetOptionW(None, INTERNET_OPTION_REFRESH, None, 0) });
        changed.and(refresh)
    }
}
