#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[macro_use]
mod i18n;
mod app;
mod cache;
mod clip;
mod color;
mod commits;
mod demo;
mod download;
mod emoji;
mod golive;
mod images;
mod login;
mod markdown;
mod message_log;
mod model;
mod net;
mod notify;
mod plugins;
mod profile;
mod sticker;
mod theme;
mod translator;
mod ui;
mod update;
mod video;
mod voice;
mod wake;

fn main() {
    if !profile::acquire() {
        eprintln!(
            "{} の Riscord はもう開いています",
            profile::label(profile::name())
        );
        return;
    }
    app::init_language();
    ui::run();
}
