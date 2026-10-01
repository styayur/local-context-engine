// Hide the console window in release builds; in debug the console is useful.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    local_context_engine_desktop_lib::run();
}
