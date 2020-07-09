#[macro_use]
extern crate log;

use ebbflow::config::{
    config_file_full, config_path_root, getkey, key_file_full, ConfigError, EbbflowDaemonConfig,
};
use ebbflow::daemon::SharedInfo;
use ebbflow::run_daemon;
use ebbflow::signal::SignalReceiver;
#[allow(unused)]
use ebbflow::{certs::ROOTS, infoserver::run_info_server, signal::SignalSender};
use futures::future::BoxFuture;
use log::LevelFilter;
use notify::{event::Event, event::EventKind, Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::Arc;
use tokio::sync::Notify;

const DEFAULT_LEVEL: LevelFilter = LevelFilter::Warn;

#[cfg(windows)]
fn main() {
    let _ = windows::run();
}

#[cfg(windows)]
mod windows {
    use ebbflow::signal::SignalSender;
    use std::{ffi::OsString, time::Duration};
    use windows_service::{
        define_windows_service,
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher, Result,
    };

    const SERVICE_NAME: &str = "ebbflowClientService";
    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

    pub fn run() -> Result<()> {
        winlog::try_register("Ebbflow Service Log").unwrap();

        // Register generated `ffi_service_main` with the system and start the service, blocking
        // this thread until the service is stopped.
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
    }

    // Generate the windows service boilerplate.
    // The boilerplate contains the low-level service entry function (ffi_service_main) that parses
    // incoming service arguments into Vec<OsString> and passes them to user defined service
    // entry (my_service_main).
    define_windows_service!(ffi_service_main, my_service_main);

    pub fn my_service_main(_arguments: Vec<OsString>) {
        if let Err(_e) = run_service() {
            // Handle the error, by logging or something.
        }
    }

    pub fn run_service() -> Result<()> {
        // Create a channel to be able to poll a stop event from the service worker loop.
        let sender = SignalSender::new();
        let r = sender.new_receiver();

        // Define system service event handler that will be receiving service events.
        let event_handler = move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                // Notifies a service to report its current status information to the service
                // control manager. Always return NoError even if not implemented.
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,

                // Handle stop
                ServiceControl::Stop => {
                    sender.send_signal();
                    ServiceControlHandlerResult::NoError
                }

                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        // Register system service event handler.
        // The returned status handle should be used to report service status changes to the system.
        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

        // Tell the system that service is running
        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(60),
            process_id: None,
        })?;

        let mut rt = tokio::runtime::Runtime::new().unwrap();

        let loglevel = rt.block_on(crate::determine_log_level());
        std::env::set_var("RUST_LOG", &loglevel.to_string());
        winlog::init("Ebbflow Service Log").unwrap();

        let existcode = rt.block_on(async move {
            match super::realmain(r).await {
                Ok(()) => {
                    info!("Daemon shutting down");
                    0
                }
                Err(e) => {
                    error!("Error in daemon {}", e);
                    1
                }
            }
        });

        // loop {
        //     // Poll shutdown event.
        //     match shutdown_rx.recv_timeout(Duration::from_secs(1)) {
        //         // Break the loop either upon stop or channel disconnect
        //         Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => break,

        //         // Continue work if no events were received within the timeout
        //         Err(mpsc::RecvTimeoutError::Timeout) => (),
        //     };
        // }

        // Tell the system that service has stopped.
        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(existcode),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        Ok(())
    }
}

#[cfg(not(windows))]
#[tokio::main]
async fn main() -> Result<(), ()> {
    let loglevel = crate::determine_log_level().await;

    env_logger::builder()
        .filter_level(loglevel)
        .filter_module("rustls", log::LevelFilter::Error) // This baby gets noisy at lower levels
        .init();

    let sender = SignalSender::new();
    let r = sender.new_receiver();

    match realmain(r).await {
        Ok(_) => {
            info!("Daemon exiting");
            Ok(())
        }
        Err(e) => {
            warn!("Daemon exited with error: {:?}", e);
            eprintln!("Daemon exited with error: {:?}", e);
            Err(())
        }
    }
}

async fn determine_log_level() -> LevelFilter {
    let leveloption = match std::env::var("EBB_LOG_LEVEL").ok() {
        Some(l) => Some(l),
        None => match config_reload().await {
            Ok((Some(cfg), _)) => cfg.loglevel,
            _ => None,
        },
    };
    use std::str::FromStr;

    match leveloption {
        Some(s) => match LevelFilter::from_str(&s) {
            Ok(lf) => lf,
            Err(_) => crate::DEFAULT_LEVEL,
        },
        None => crate::DEFAULT_LEVEL,
    }
}

async fn realmain(mut wait: SignalReceiver) -> Result<(), String> {
    info!("Config file dir {}", config_path_root());

    let notify = Arc::new(Notify::new());
    let notifyc = notify.clone();
    let mut watcher: RecommendedWatcher =
        match Watcher::new_immediate(move |res: Result<Event, notify::Error>| {
            trace!("Received a notification");
            match res {
                Ok(event) => match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) => {
                        debug!("Received a notication for create or modify");
                        notifyc.notify();
                    }
                    _ => {
                        trace!(
                            "Received notication for an event that we don't care about {:#?}",
                            event
                        );
                    }
                },
                Err(e) => {
                    panic!("Error listening for file events {:?}", e);
                }
            }
        }) {
            Ok(x) => x,
            Err(e) => return Err(format!("Error creating new file watcher {:?}", e)),
        };

    // We only care about mutations
    if let Err(e) = watcher.configure(Config::PreciseEvents(true)) {
        return Err(format!(
            "Unable to set file event configuration options (precise) {:?}",
            e
        ));
    }

    // Add a path to be watched. All files and directories at that path and
    // below will be monitored for changes.
    if let Err(e) = watcher.watch(config_path_root(), RecursiveMode::Recursive) {
        return Err(format!(
            "Unable to set file event configuration options {:?}",
            e
        ));
    }

    let sharedinfo = match (
        std::env::var("UNSTABLE_EBB_ADDR").ok(),
        std::env::var("UNSTABLE_EBB_DNS").ok(),
    ) {
        (Some(addr), Some(dns)) => {
            let addr = match addr.parse() {
                Ok(x) => x,
                Err(e) => {
                    return Err(format!(
                        "Error creating ipv4 addr from overriden value {} {:?}",
                        addr, e
                    ))
                }
            };
            Arc::new(
                match SharedInfo::new_with_ebbflow_overrides(addr, dns.to_string(), ROOTS.clone())
                    .await
                {
                    Ok(x) => x,
                    Err(e) => return Err(format!("Error creating daemon settings {:?}", e)),
                },
            )
        }
        _ => Arc::new(match SharedInfo::new().await {
            Ok(x) => x,
            Err(e) => return Err(format!("Error creating daemon settings {:?}", e)),
        }),
    };

    let runner = run_daemon(sharedinfo, Box::pin(config_reload), notify).await;
    let runnerc = runner.clone();

    // Spawn the server that produces info about the daemon
    tokio::spawn(run_info_server(runnerc));

    wait.wait().await;
    Ok(())
}

pub fn config_reload(
) -> BoxFuture<'static, Result<(Option<EbbflowDaemonConfig>, Option<String>), ConfigError>> {
    Box::pin(async {
        debug!("Will read config file, {}", config_file_full());
        let cfg = match EbbflowDaemonConfig::load_from_file().await {
            Ok(c) => Some(c),
            Err(ref e) if &ConfigError::Empty == e => None,
            Err(e) => return Err(e),
        };
        debug!(
            "Config file parsed successfully, now trying key file {}",
            key_file_full()
        );
        let key = match getkey().await {
            Ok(k) => Some(k),
            Err(e) => match e {
                ConfigError::Empty | ConfigError::FileNotFound => None,
                _ => return Err(e),
            },
        };
        debug!("Found key: {}", key.is_some());
        Ok((cfg, key))
    })
}
