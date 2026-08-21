// No console window in the Windows release build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    clippiboy_lib::run()
}
