// Release builds must not open a console window behind the application.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    blinkify_desktop_lib::run();
}
