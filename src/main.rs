#![windows_subsystem = "windows"]

mod accounts;
mod app_settings;
mod context_menu;
mod dashboard;
mod desktop_compositor;
mod diagnose;
mod font_catalog;
#[cfg(test)]
mod https_test;
mod localization;
mod models;
mod monitors;
mod native_interop;
mod poller;
mod providers;
mod startup;
mod studio_app;
mod theme;
mod theme_engine;
mod theme_package;
mod tray_icon;
mod ui;
mod updater;
mod window;
mod winsqlite;

fn main() {
    diagnose::install_panic_hook();

    let args: Vec<String> = std::env::args().collect();
    let diagnose_enabled = args.iter().any(|arg| arg == "--diagnose");
    if diagnose_enabled {
        let init_result = if args.iter().any(|arg| arg == "--diagnose-append") {
            diagnose::init_append()
        } else {
            diagnose::init()
        };
        match init_result {
            Ok(path) => diagnose::log(format!("startup args={args:?} log_path={}", path.display())),
            Err(error) => {
                // Logging may not be available yet, but keep startup behavior unchanged.
                let _ = error;
            }
        }
    }

    if let Some(index) = args.iter().position(|arg| arg == "--import-v1") {
        let result = args
            .get(index + 1)
            .filter(|value| !value.starts_with("--"))
            .ok_or_else(|| "--import-v1 requires a snapshot directory.".to_string())
            .and_then(|directory| {
                app_settings::import_legacy_profile(std::path::Path::new(directory))
            });
        match result {
            Ok(imported) => diagnose::log(format!("v1 snapshot imported={imported}")),
            Err(error) => {
                diagnose::log_error("v1 snapshot import failed", &error);
                use windows::core::{w, PCWSTR};
                use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
                let message = native_interop::wide_str(&error);
                unsafe {
                    MessageBoxW(
                        None,
                        PCWSTR(message.as_ptr()),
                        w!("Hardened v2 experiment: import failed"),
                        MB_OK | MB_ICONERROR,
                    );
                }
                std::process::exit(2);
            }
        }
    }

    if studio_app::handle_cli_mode(&args) {
        return;
    }

    if diagnose_enabled {
        diagnose::log("entering window::run");
    }
    window::run();
}
