//! Compile-time optional-analysis boundary for shipping targets.
//!
//! This is deliberately not hardware detection or a recommendation layer.
//! A target enters the supported set only after its packaged implementation
//! passes the physical acceptance contract; until then the code, dependency
//! graph, model acquisition, and interface all agree that it is unavailable.

pub const FACE_SCORING: bool = cfg!(all(target_os = "macos", target_arch = "aarch64"));
pub const TRANSCRIPTION: bool = cfg!(all(target_os = "macos", target_arch = "aarch64"));

pub const MAC_ONLY_REASON: &str = "Currently available only on Apple silicon Macs";

pub fn managed_dependency_supported(id: &str) -> bool {
    match id {
        "whisper-large-v3-turbo" => TRANSCRIPTION,
        "ultraface-rfb640" | "hsemotion-enet-b2" => FACE_SCORING,
        _ => true,
    }
}
