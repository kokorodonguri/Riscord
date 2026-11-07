#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod clip;
mod color;
mod demo;
mod images;
mod login;
mod model;
mod net;
mod sticker;
mod theme;
mod ui;
mod voice;
mod wake;

fn main() {
    ui::run();
}
