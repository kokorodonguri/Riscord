//! UI-independent modules shared with the desktop client.
#[path = "../../../src/model.rs"]
pub mod model;
#[path = "../../../src/wake.rs"]
pub mod wake;
#[path = "../../../src/login.rs"]
pub mod login;

#[path = "../../../src/net/inflate.rs"]
mod inflate;
#[path = "../../../src/net/rest.rs"]
pub mod rest;
pub mod mobile;
mod android;

#[cfg(target_os = "android")]
#[path = "../../../src/voice.rs"]
pub mod voice;
#[cfg(target_os = "android")]
mod mobile_voice;
