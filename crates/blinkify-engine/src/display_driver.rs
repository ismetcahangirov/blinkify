//! The driver version of every display adapter on this machine.
//!
//! A hardware encoder is the GPU driver's, not the sidecar's (`ADR-0003`): a
//! new NVIDIA, Intel or AMD driver can add a profile, raise a level ceiling or
//! break an encoder that worked yesterday. So the driver versions are part of
//! the key the encoder capability profile is cached under (#83), and a new
//! driver means a new probe.
//!
//! They are read from the display adapter device class in the registry —
//! `HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-…}\NNNN`, one
//! numbered subkey per installed adapter, each with `DriverDesc` and
//! `DriverVersion`. That is where the driver's INF writes them, where Device
//! Manager and WMI's `Win32_VideoController.DriverVersion` read them, and a
//! standard user may read it. No process is started and no shell is involved.
//! `docs/architecture/encoder-capability-cache.md` records what was rejected.

use serde::{Deserialize, Serialize};

/// One display adapter and the version of the driver installed for it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayDriver {
    /// The adapter as its driver names it: `NVIDIA GeForce RTX 3060`.
    pub adapter: String,
    /// The driver's four-part version: `32.0.15.7688`.
    pub version: String,
}

/// The display adapter device class, `GUID_DEVCLASS_DISPLAY`.
#[cfg(windows)]
const DISPLAY_CLASS: &str =
    r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

/// Every display adapter's driver, sorted, so the same machine always gives
/// the same list.
///
/// An adapter whose key cannot be read is left out rather than failing the
/// whole list. The list is a cache key, and the cost of a key that is
/// incomplete is one extra probe when it later becomes complete — never a
/// stale answer, because anything that changes the list changes the key.
#[cfg(windows)]
#[must_use]
pub fn installed() -> Vec<DisplayDriver> {
    use windows_registry::LOCAL_MACHINE;

    let Ok(class) = LOCAL_MACHINE.open(DISPLAY_CLASS) else {
        return Vec::new();
    };
    let Ok(names) = class.keys() else {
        return Vec::new();
    };
    let mut drivers: Vec<DisplayDriver> = names
        // Adapters are `0000`, `0001` …; `Properties` and `Configuration`
        // sit beside them and are not adapters.
        .filter(|name| is_adapter_subkey(name))
        .filter_map(|name| {
            let adapter = class.open(&name).ok()?;
            Some(DisplayDriver {
                version: adapter.get_string("DriverVersion").ok()?,
                adapter: adapter.get_string("DriverDesc").unwrap_or_default(),
            })
        })
        .collect();
    drivers.sort();
    drivers
}

/// Outside Windows there is no hardware encoder Blinkify uses, and no driver
/// to key on.
#[cfg(not(windows))]
#[must_use]
pub fn installed() -> Vec<DisplayDriver> {
    Vec::new()
}

/// Whether a subkey name under the device class is an adapter instance: four
/// decimal digits.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_adapter_subkey(name: &str) -> bool {
    name.len() == 4 && name.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn only_numbered_subkeys_are_adapters() {
        assert!(is_adapter_subkey("0000"));
        assert!(is_adapter_subkey("0013"));
        assert!(!is_adapter_subkey("Properties"));
        assert!(!is_adapter_subkey("Configuration"));
        assert!(!is_adapter_subkey("000"));
        assert!(!is_adapter_subkey("00000"));
    }

    #[test]
    fn the_same_machine_gives_the_same_list() {
        let first = installed();
        assert_eq!(first, installed());
        assert!(first.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(first.iter().all(|driver| !driver.version.is_empty()));
    }
}
