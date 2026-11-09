//! Microphone discovery shared by the capture path and the CLI.

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};

/// Shown when the system has no microphone at all, the out-of-box state of
/// a Mac mini, Studio or Pro.
pub const NO_MICROPHONE: &str = "No microphone found. Connect a microphone and try again.";

/// The parts of an audio host that device selection reads, so the selection
/// rules can be tested without audio hardware.
trait InputHost {
    type Device;
    fn default_input(&self) -> Option<Self::Device>;
    fn inputs(&self) -> Result<Vec<Self::Device>>;
    fn device_name(device: &Self::Device) -> Option<String>;
}

impl InputHost for cpal::Host {
    type Device = cpal::Device;

    fn default_input(&self) -> Option<cpal::Device> {
        self.default_input_device()
    }

    fn inputs(&self) -> Result<Vec<cpal::Device>> {
        Ok(self
            .input_devices()
            .context("Failed to enumerate input devices")?
            .collect())
    }

    fn device_name(device: &cpal::Device) -> Option<String> {
        device.name().ok()
    }
}

/// The input device named `preferred`, falling back to the system default
/// when it is unset or no longer connected.
///
/// # Errors
/// [`NO_MICROPHONE`] when there is no default microphone to fall back to, or
/// an enumeration error when a preferred device is set but the device list
/// can't be read.
pub fn select_input_device(host: &cpal::Host, preferred: Option<&str>) -> Result<cpal::Device> {
    select(host, preferred)
}

// On macOS a machine with no microphone reports kAudioObjectUnknown (ID 0) as
// its default input, and cpal 0.15 wraps that as a device instead of returning
// None; every query on it then fails with an opaque CoreAudio error. A default
// that can't even report its name is that phantom, so it counts as absent.
fn present_default<H: InputHost>(host: &H) -> Option<H::Device> {
    host.default_input()
        .filter(|device| H::device_name(device).is_some())
}

fn select<H: InputHost>(host: &H, preferred: Option<&str>) -> Result<H::Device> {
    if let Some(preferred_name) = preferred.map(str::trim).filter(|name| !name.is_empty()) {
        let found = host
            .inputs()?
            .into_iter()
            .find(|device| H::device_name(device).as_deref() == Some(preferred_name));
        if let Some(device) = found {
            return Ok(device);
        }
        tracing::warn!(
            "Preferred input device {:?} not found, falling back to default input device",
            preferred_name
        );
    }

    present_default(host).ok_or_else(|| anyhow::anyhow!(NO_MICROPHONE))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device as discovery sees it; `None` is one that can't report a name.
    #[derive(Debug, Clone, PartialEq)]
    struct FakeDevice(Option<&'static str>);

    struct FakeHost {
        default: Option<FakeDevice>,
        inputs: Vec<FakeDevice>,
    }

    impl InputHost for FakeHost {
        type Device = FakeDevice;

        fn default_input(&self) -> Option<FakeDevice> {
            self.default.clone()
        }

        fn inputs(&self) -> Result<Vec<FakeDevice>> {
            Ok(self.inputs.clone())
        }

        fn device_name(device: &FakeDevice) -> Option<String> {
            device.0.map(str::to_owned)
        }
    }

    /// A Mac with no microphone as cpal 0.15 reports it: a default input
    /// that exists but can't answer queries, and an empty input list.
    fn mac_without_microphone() -> FakeHost {
        FakeHost {
            default: Some(FakeDevice(None)),
            inputs: Vec::new(),
        }
    }

    fn host_with_built_in_and_usb() -> FakeHost {
        let built_in = FakeDevice(Some("Built-in Microphone"));
        FakeHost {
            default: Some(built_in.clone()),
            inputs: vec![built_in, FakeDevice(Some("USB Mic"))],
        }
    }

    #[test]
    fn phantom_default_counts_as_no_microphone() {
        assert_eq!(present_default(&mac_without_microphone()), None);
    }

    #[test]
    fn selecting_without_a_microphone_says_so() {
        let err = select(&mac_without_microphone(), None).unwrap_err();
        assert_eq!(err.to_string(), NO_MICROPHONE);
    }

    #[test]
    fn unplugged_preferred_device_without_a_microphone_says_so() {
        let err = select(&mac_without_microphone(), Some("USB Mic")).unwrap_err();
        assert_eq!(err.to_string(), NO_MICROPHONE);
    }

    #[test]
    fn preferred_device_is_chosen_by_name() {
        let device = select(&host_with_built_in_and_usb(), Some(" USB Mic ")).unwrap();
        assert_eq!(device, FakeDevice(Some("USB Mic")));
    }

    #[test]
    fn unplugged_preferred_device_falls_back_to_the_default() {
        let device = select(&host_with_built_in_and_usb(), Some("Gone")).unwrap();
        assert_eq!(device, FakeDevice(Some("Built-in Microphone")));
    }
}
