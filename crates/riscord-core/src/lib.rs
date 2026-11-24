//! UI-independent modules shared with the desktop client.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]
#[macro_use]
#[path = "../../../src/i18n.rs"]
pub mod i18n;
#[path = "../../../src/login.rs"]
pub mod login;
#[path = "../../../src/model.rs"]
pub mod model;
#[path = "../../../src/wake.rs"]
pub mod wake;

mod android;
// デスクトップだけで使う関数（close_command など）もある
#[allow(dead_code)]
#[path = "../../../src/gateway.rs"]
mod gateway;
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
