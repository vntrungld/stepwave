//! Platform-neutral device-name checks used by `wasapi_io`.

/// Whether `name` (an endpoint's friendly name) is VB-Cable's own playback endpoint. stepwave
/// must never render into it: CS2 already plays into "CABLE Input", so rendering there too
/// would feed stepwave's output back into its own capture.
pub fn is_virtual_cable(name: &str) -> bool {
    let name = name.to_lowercase();
    name.contains("cable input") || name.contains("vb-audio")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_vb_cable_playback_endpoints() {
        assert!(is_virtual_cable("CABLE Input (VB-Audio Virtual Cable)"));
        assert!(is_virtual_cable("cable input"));
        assert!(is_virtual_cable("Speakers (VB-Audio Virtual Cable)"));
    }

    #[test]
    fn accepts_real_devices() {
        assert!(!is_virtual_cable("Headphones (HyperX Cloud II)"));
        assert!(!is_virtual_cable("Speakers (Realtek(R) Audio)"));
        assert!(!is_virtual_cable("Cable Guy Headset"));
    }
}
