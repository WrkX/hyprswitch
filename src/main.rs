use std::error::Error;
use std::process::exit;
use std::sync::Mutex;

use anyhow::Context;
use clap::Parser;
use gtk4::IconTheme;
use hyprswitch::cli::{App, SwitchType};
use hyprswitch::client::{daemon_running, send_close_daemon, send_init_command, send_switch_command};
use hyprswitch::daemon::{deactivate_submap, get_desktop_files_debug, get_icon_name_debug, start_daemon};
use hyprswitch::handle::{collect_data, find_next, switch_to_active};
use hyprswitch::{check_version, cli, Active, Command, Config, GuiConfig, ACTIVE, DRY};
use log::{debug, info, trace, warn};
use notify_rust::{Notification, Urgency};


fn main() -> Result<(), Box<dyn Error>> {
    let cli = App::try_parse()
        .unwrap_or_else(|e| {
            // only show error if not caused by --help ort -V (every start of every help text needs to be added...)
            if !(e.to_string().starts_with("A CLI/GUI that allows switching between windows in Hyprland") ||
                e.to_string().starts_with("Opens the GUI") ||
                e.to_string().starts_with("Initialize and start the Daemon") ||
                e.to_string().starts_with("Used to send commands to the daemon (used in keymap that gets generated by gui)") ||
                e.to_string().starts_with("Switch without using the GUI / Daemon (switches directly)") ||
                e.to_string().starts_with("Close the GUI, executes the command to switch window") || e.to_string() == format!("hyprswitch {}\n", option_env!("CARGO_PKG_VERSION").unwrap_or("?.?.?"))) {
                let _ = Notification::new()
                    .summary(&format!("Hyprswitch ({}) Error", option_env!("CARGO_PKG_VERSION").unwrap_or("?.?.?")))
                    .body("Unable to parse CLI Arguments (visit https://github.com/H3rmt/hyprswitch/blob/main/README.md to see all CLI Args)")
                    .timeout(10000)
                    .hint(notify_rust::Hint::Urgency(Urgency::Critical))
                    .show();
            }
            eprintln!("{}", e);
            exit(1);
        });
    stderrlog::new().module(module_path!()).verbosity(cli.global_opts.verbose as usize + 1).init()
        .context("Failed to initialize logging :(").unwrap_or_else(|e| warn!("{:?}", e));

    let _ = check_version().map_err(|e| {
        warn!("Unable to check Hyprland version, continuing anyway");
        debug!("{:?}", e);
    });

    DRY.set(cli.global_opts.dry_run).expect("unable to set DRY (already filled???)");
    ACTIVE.set(Mutex::new(false)).expect("unable to set ACTIVE (already filled???)");

    match cli.command {
        cli::Command::Init { custom_css, show_title, workspaces_per_row, size_factor } => {
            if daemon_running() {
                warn!("Daemon already running");
                return Ok(());
            }
            info!("Starting daemon");
            start_daemon(custom_css, show_title, size_factor, workspaces_per_row)
                .context("Failed to run daemon")
                .inspect_err(|_| {
                    let _ = deactivate_submap();
                })?;
            return Ok(());
        }
        cli::Command::Close { kill } => {
            info!("Stopping daemon");

            if !daemon_running() {
                warn!("Daemon not running");
                return Ok(());
            }
            send_close_daemon(kill).context("Failed to send kill command to daemon")?;
        }
        cli::Command::Dispatch { simple_opts } => {
            let command = Command::from(simple_opts);
            send_switch_command(command)
                .with_context(|| format!("Failed to send switch command with command {command:?} to daemon"))?;
        }
        cli::Command::Gui { gui_conf, simple_config } => {
            if !daemon_running() {
                let _ = Notification::new()
                    .summary(&format!("Hyprswitch ({}) Error", option_env!("CARGO_PKG_VERSION").unwrap_or("?.?.?")))
                    .body("Daemon not running (add ``exec-once = hyprswitch init &`` to your Hyprland config or run ``hyprswitch init &`` it in a terminal)\nvisit https://github.com/H3rmt/hyprswitch/wiki/Examples to see Example configs")
                    .timeout(10000)
                    .hint(notify_rust::Hint::Urgency(Urgency::Critical))
                    .show();
                return Err(Box::from(anyhow::anyhow!("Daemon not running")));
            }

            // Daemon is not running
            info!("initialising daemon");
            let config = Config::from(simple_config);
            let gui_config = GuiConfig::from(gui_conf);
            send_init_command(config.clone(), gui_config.clone())
                .with_context(|| format!("Failed to send init command with config {config:?} and gui_config {gui_config:?} to daemon"))?;

            return Ok(());
        }
        cli::Command::Simple { simple_opts, simple_conf } => {
            let config = Config::from(simple_conf);
            let (clients_data, active) = collect_data(config.clone()).with_context(|| format!("Failed to collect data with config {config:?}"))?;
            trace!("Clients data: {:?}", clients_data);

            let command = Command::from(simple_opts);

            let active = match config.switch_type {
                SwitchType::Client => if let Some(add) = active.0 { Active::Client(add) } else { Active::Unknown },
                SwitchType::Workspace => if let Some(ws) = active.1 { Active::Workspace(ws) } else { Active::Unknown },
                SwitchType::Monitor => if let Some(mon) = active.2 { Active::Monitor(mon) } else { Active::Unknown },
            };
            info!("Active: {:?}", active);
            let next_active = find_next(&config.switch_type, command, &clients_data, &active);
            if let Ok(next_active) = next_active {
                switch_to_active(&next_active, &clients_data)?;
            }
        }
        cli::Command::Icon { class, desktop_files, list } => {
            println!("use with -vvv icon ... to see full logs!");
            match (list, desktop_files) {
                (true, false) => {
                    gtk4::init().context("Failed to init gtk")?;
                    let theme = IconTheme::new();
                    for icon in theme.icon_names() {
                        info!("[ICON] Icon: {icon}");
                    }
                }
                (false, true) => {
                    let map = get_desktop_files_debug();

                    for (name, file) in map {
                        info!("[ICON] Desktop file: {name} -> {} ({})", file.0, match file.1 {
                            0 => "Name",
                            1 => "Exec",
                            2 => "StartupWMClass",
                            _ => "Unknown",
                        });
                    }
                }
                _ => {
                    info!("[ICON] Icon for class {class}");
                    gtk4::init().context("Failed to init gtk")?;
                    let theme = IconTheme::new();
                    if theme.has_icon(&class) {
                        info!("[ICON] Theme contains icon for class {class}");
                    } else {
                        info!("[ICON] Theme does not contain icon for class {class}");
                        let name = get_icon_name_debug(&class)
                            .with_context(|| format!("Failed to get icon name for class {class}"))?;
                        info!("[ICON] name from desktop file: {} from {}", name.0, match name.1 {
                            0 => "Name",
                            1 => "Exec",
                            2 => "StartupWMClass",
                            _ => "Unknown",
                        });
                        if theme.has_icon(&name.0) {
                            info!("[ICON] Theme contains icon for name {}", name.0);
                        } else {
                            info!("[ICON] Theme does not contain icon for name {}", name.0);
                        }
                    }
                }
            }
        }
    };
    Ok(())
}