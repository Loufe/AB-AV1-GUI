//! Thin entry point; all wiring lives in the crfty-shell library.
#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    crfty_shell::run();
}
