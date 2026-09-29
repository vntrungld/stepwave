//! Platform-neutral stepwave runtime shared by the Linux daemon and the Windows app:
//! lock-free processor swaps (`audio`), control state (`engine`), the JSON-line control
//! protocol (`protocol`) and profile loading (`profiles`).

pub mod audio;
pub mod engine;
pub mod profiles;
pub mod protocol;
