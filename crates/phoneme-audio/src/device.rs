//! Audio input device enumeration via CPAL.
//!
//! On Windows this is the WASAPI default-host's input devices.

use cpal::traits::{DeviceTrait, HostTrait};
use phoneme_core::error::{Error, Result};

/// Lightweight info about an audio input device.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceInfo {
    /// Host-reported device name. This doubles as the device's identity: it is
    /// what the config stores and what [`resolve_input_device`] looks up.
    pub name: String,
    /// Whether this device is the host's current default input.
    pub is_default: bool,
}

/// Enumerate available input devices on the system default host.
pub fn list_input_devices() -> Result<Vec<DeviceInfo>> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|d| d.name().ok());

    let devices = host
        .input_devices()
        .map_err(|e| Error::Internal(format!("cpal input_devices: {e}")))?;

    let mut out = Vec::new();
    for d in devices {
        let name = match d.name() {
            Ok(n) => n,
            Err(_) => continue, // skip devices we can't name
        };
        let is_default = default_name.as_deref() == Some(&name);
        out.push(DeviceInfo { name, is_default });
    }
    Ok(out)
}

/// Return the system default input device, if any.
pub fn default_input_device() -> Option<DeviceInfo> {
    let host = cpal::default_host();
    let d = host.default_input_device()?;
    let name = d.name().ok()?;
    Some(DeviceInfo {
        name,
        is_default: true,
    })
}

/// Resolve a device by name. Returns `None` if not found.
pub fn find_device_by_name(name: &str) -> Result<Option<cpal::Device>> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|e| Error::Internal(format!("cpal input_devices: {e}")))?;
    for d in devices {
        if d.name().ok().as_deref() == Some(name) {
            return Ok(Some(d));
        }
    }
    Ok(None)
}

/// Resolve the configured input down to a concrete CPAL device.
///
/// `"default"` or an empty string selects the host default; anything else is
/// matched against device names. Returns `Err` when the default is requested but
/// the host reports none, or when a named device can't be found (so a stale
/// config naming an unplugged device surfaces a clear error rather than silently
/// falling back).
pub fn resolve_input_device(requested: &str) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if requested == "default" || requested.is_empty() {
        return host
            .default_input_device()
            .ok_or_else(|| Error::Internal("no default input device".into()));
    }
    find_device_by_name(requested)?
        .ok_or_else(|| Error::Internal(format!("input device not found: {requested}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests need a real audio device on the system. Skip if none is
    // present — CI runners (especially Linux containers) often lack one.
    fn has_audio() -> bool {
        if std::env::var("CI").is_ok() {
            return false;
        }
        default_input_device().is_some()
    }

    #[test]
    fn list_input_devices_does_not_panic() {
        if !has_audio() {
            return;
        }
        // We don't assert a particular count; just that the call succeeds.
        let _ = list_input_devices();
    }

    #[test]
    fn default_marker_aligns_with_helper() {
        if !has_audio() {
            return;
        }
        let list = list_input_devices().unwrap();
        let default_count = list.iter().filter(|d| d.is_default).count();
        assert!(default_count <= 1, "at most one default expected");
        let default = default_input_device().unwrap();
        let in_list = list.iter().any(|d| d.name == default.name);
        assert!(in_list, "default should appear in the list");
    }

    #[test]
    fn resolve_default_returns_some_device() {
        if !has_audio() {
            return;
        }
        let _device = resolve_input_device("default").unwrap();
    }

    #[test]
    fn resolve_unknown_name_errors() {
        // Can't use `.unwrap_err()` because cpal::Device doesn't impl Debug.
        let result = resolve_input_device("absolutely_not_a_real_device_xyz");
        let err = result.err().expect("expected error for unknown device");
        assert!(format!("{err}").contains("not found"));
    }
}
