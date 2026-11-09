#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod cache;
mod clip;
mod color;
mod demo;
mod images;
mod login;
mod model;
mod net;
mod notify;
mod sticker;
mod theme;
mod ui;
mod update;
mod voice;
mod wake;

fn main() {
    ui::run();
}
