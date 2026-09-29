//! Platform-neutral device-name checks used by `wasapi_io`.

/// Whether `name` (an endpoint's friendly name) is VB-Cable's own playback endpoint. stepwave
/// must never render into it: CS2 already plays into "CABLE Input", so rendering there too
/// would feed stepwave's output back into its own capture ("CABLE Output").
///
/// Only the plain VB-Cable ("VB-Audio Virtual Cable") matches. Other VB-Audio devices —
/// Voicemeeter, Hi-Fi Cable, Cable A/B — are separate cables that do not feed CABLE Output,
/// so they are valid outputs.
pub fn is_virtual_cable(name: &str) -> bool {
    let name = name.to_lowercase();
    name.contains("vb-audio virtual cable") || name.starts_with("cable input")
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
    fn accepts_other_vb_audio_devices() {
        assert!(!is_virtual_cable(
            "VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)"
        ));
        assert!(!is_virtual_cable(
            "VoiceMeeter Aux Input (VB-Audio VoiceMeeter AUX VAIO)"
        ));
        assert!(!is_virtual_cable(
            "Hi-Fi Cable Input (VB-Audio Hi-Fi Cable)"
        ));
        assert!(!is_virtual_cable("CABLE-A Input (VB-Audio Cable A)"));
    }

    #[test]
    fn accepts_real_devices() {
        assert!(!is_virtual_cable("Headphones (HyperX Cloud II)"));
        assert!(!is_virtual_cable("Speakers (Realtek(R) Audio)"));
        assert!(!is_virtual_cable("Cable Guy Headset"));
    }
}
