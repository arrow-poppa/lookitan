// Lookitan: OCR pop-up dictionary with bring-your-own-key AI.
// Copyright (C) 2026 arrow-poppa. Licensed under the GNU GPL v3 only.

// No console window behind the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    lookitan_lib::run()
}
