#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod cache;
mod clip;
mod color;
mod demo;
mod download;
mod emoji;
mod images;
mod login;
mod model;
mod net;
mod notify;
mod plugins;
mod sticker;
mod theme;
mod translator;
mod ui;
mod update;
mod voice;
mod video;
mod wake;

fn main() {
    ui::run();
}
