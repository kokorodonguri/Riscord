//! UI-independent modules shared with the desktop client.
#[path = "../../../src/login.rs"]
pub mod login;
#[path = "../../../src/model.rs"]
pub mod model;
#[path = "../../../src/wake.rs"]
pub mod wake;

mod android;
#[path = "../../../src/net/inflate.rs"]
mod inflate;
pub mod mobile;
#[path = "../../../src/net/rest.rs"]
pub mod rest;

#[cfg(target_os = "android")]
mod mobile_voice;
#[cfg(target_os = "android")]
#[path = "../../../src/voice.rs"]
pub mod voice;
