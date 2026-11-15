//! Glowdrift desktop shell: Tauri application wiring and IPC commands.

pub mod commands;
pub mod config_store;
pub mod engine;
pub mod frame_source;
pub mod hue;
pub mod hue_commands;
pub mod hue_store;
pub mod icue_sdk;
pub mod lights_manager;
pub mod per_machine;
mod timing;
pub mod tray;
pub mod updater;
pub mod window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glowdrift_core::Rgb;
use log::LevelFilter;
use tauri::menu::MenuItem;
use tauri::{AppHandle, Emitter, Manager, RunEvent, Wry};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

use commands::{AppState, ColorEvent, events};
use config_store::ConfigStore;
use engine::{EngineEvents, EngineHandle, EngineStatus};
use hue::HueState;
use lights_manager::{LightsManager, default_lights};

/// Argument passed by the autostart entry.
const MINIMIZED_ARG: &str = "--minimized";
/// Argument that asks the running instance to quit. A process started with it while no other
/// instance is running exits immediately.
const QUIT_ARG: &str = "--quit";
/// If the frontend never reports being ready, a window that should be visible is shown anyway.
const SHOW_FALLBACK_DELAY: Duration = Duration::from_secs(4);
const LOG_FILE_NAME: &str = "glowdrift";
const LOG_MAX_FILE_BYTES: u128 = 1_000_000;
const LOG_KEEP_FILES: usize = 5;

/// Forwards engine notifications to the frontend and keeps the tray label in sync.
struct TauriEvents {
    app: AppHandle,
    pause_item: MenuItem<Wry>,
}

impl EngineEvents for TauriEvents {
    fn color(&self, target: Rgb, smoothed: Rgb, zones: &[Rgb]) {
        let payload = ColorEvent {
            target: target.to_hex(),
            smoothed: smoothed.to_hex(),
            zones: zones.iter().map(|zone| zone.to_hex()).collect(),
        };
        let _ = self.app.emit(events::COLOR, payload);
    }

    fn status(&self, status: &EngineStatus) {
        let _ = self.app.emit(events::ENGINE_STATUS, status);
        let item = self.pause_item.clone();
        let label = tray::pause_label(status.running);
        // Dispatched asynchronously so the engine thread never waits on the main thread.
        let _ = self.app.run_on_main_thread(move || {
            let _ = item.set_text(label);
        });
    }
}

fn log_plugin() -> tauri::plugin::TauriPlugin<Wry> {
    let debug = cfg!(debug_assertions);
    let mut builder = tauri_plugin_log::Builder::new()
        .level(if debug {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        })
        .level_for("tauri_plugin_updater", LevelFilter::Off)
        .max_file_size(LOG_MAX_FILE_BYTES)
        .rotation_strategy(RotationStrategy::KeepSome(LOG_KEEP_FILES))
        .clear_targets()
        .target(Target::new(TargetKind::LogDir {
            file_name: Some(LOG_FILE_NAME.to_owned()),
        }));
    if debug {
        builder = builder.target(Target::new(TargetKind::Stdout));
    }
    builder.build()
}

fn has_arg(arg: &str) -> bool {
    std::env::args().any(|a| a == arg)
}

fn is_quitting(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .is_some_and(|state| state.is_quitting())
}

fn initialise(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    if has_arg(QUIT_ARG) {
        // The single-instance plugin forwards the flag to a running instance and exits this
        // process; reaching this point means there was no running instance to quit.
        std::process::exit(0);
    }
    let handle = app.handle().clone();
    log::info!("starting Glowdrift {}", env!("CARGO_PKG_VERSION"));

    let store = ConfigStore::new(app.path().app_config_dir()?);
    let config = store.load();

    let (hue_light, hue) = HueState::start(app.path().app_data_dir()?, &config.lights.hue);
    let emit_handle = handle.clone();
    let lights = LightsManager::new(
        default_lights(&handle, hue_light),
        &config.lights,
        move |statuses| {
            let _ = emit_handle.emit(events::LIGHTS_STATUS, statuses);
        },
    );
    lights.start_poller();

    let pause_item = tray::build(&handle)?;
    let engine = EngineHandle::spawn(
        config.clone(),
        Arc::clone(&lights),
        TauriEvents {
            app: handle.clone(),
            pause_item,
        },
    )?;

    let launched_by_autostart = has_arg(MINIMIZED_ARG);
    let start_hidden =
        launched_by_autostart && config.start_minimized && config.onboarding_complete;

    app.manage(AppState {
        config: Mutex::new(config),
        store,
        engine,
        lights,
        hue,
        pending_update: Mutex::new(None),
        show_on_load: AtomicBool::new(!start_hidden),
        quitting: AtomicBool::new(false),
    });

    if !start_hidden {
        let fallback = handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(SHOW_FALLBACK_DELAY);
            show_initial_window(&fallback);
        });
    }
    updater::spawn_scheduler(handle);
    Ok(())
}

/// Shows the main window once, the first time this is called while it is still pending.
pub(crate) fn show_initial_window(app: &AppHandle) {
    let pending = app
        .try_state::<AppState>()
        .is_some_and(|state| state.show_on_load.swap(false, Ordering::AcqRel));
    if pending {
        window::show_main(app);
    }
}

/// Builds and runs the Tauri application.
///
/// # Panics
/// Panics if the Tauri runtime fails to start.
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if args.iter().any(|arg| arg == QUIT_ARG) {
                log::info!("quit requested by a second instance");
                tray::quit(app);
            } else {
                log::info!("second instance launched; showing the main window");
                window::show_main(app);
            }
        }))
        .plugin(log_plugin())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![MINIMIZED_ARG]),
        ))
        .on_window_event(|win, event| {
            if win.label() != window::MAIN_LABEL {
                return;
            }
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    if !is_quitting(win.app_handle()) {
                        api.prevent_close();
                        window::hide_main(win.app_handle());
                    }
                }
                tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Focused(_) => {
                    window::sync_preview(win.app_handle());
                }
                _ => {}
            }
        })
        .setup(|app| initialise(app))
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::set_config,
            commands::list_monitors,
            commands::get_monitor_thumbnail,
            commands::get_engine_status,
            commands::set_engine_running,
            commands::list_lights,
            commands::get_app_info,
            commands::install_pending_update,
            commands::update_needs_elevation,
            commands::install_icue_sdk,
            commands::frontend_ready,
            commands::stage_update,
            hue_commands::hue_discover,
            hue_commands::hue_probe,
            hue_commands::hue_pair,
            hue_commands::hue_bridge,
            hue_commands::hue_list_areas,
            hue_commands::hue_select_area,
            hue_commands::hue_forget,
            hue_commands::hue_area_lights,
            hue_commands::hue_list_lights,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Glowdrift");

    app.run(|app, event| match event {
        RunEvent::ExitRequested {
            code: None, api, ..
        } if !is_quitting(app) => api.prevent_exit(),
        RunEvent::Exit => {
            if let Some(state) = app.try_state::<AppState>() {
                state.shutdown();
            }
        }
        _ => {}
    });
}
