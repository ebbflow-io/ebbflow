#[macro_use]
extern crate log;

use ebbflow::config::{ConfigError, EbbflowDaemonConfig};
use ebbflow::daemon::SharedInfo;
use ebbflow::run_daemon;
use futures::future::BoxFuture;
use notify::{event::Event, event::EventKind, Config, RecommendedWatcher, RecursiveMode, Watcher};
use rustls::RootCertStore;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

#[cfg(windows)]
#[macro_use]
extern crate windows_service;

#[cfg(windows)]
fn main() {
    println!("hi");
    windows::run();
    //realmain().await;
}

#[cfg(windows)]
mod windows {

    use std::{
        ffi::OsString,
        sync::mpsc,
        time::Duration,
    };
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
        std::env::set_var("RUST_LOG", "INFO");
        winlog::init("Ebbflow Service Log").unwrap();
        info!("Hello, Event Log");
        // Create a channel to be able to poll a stop event from the service worker loop.
        let (shutdown_tx, shutdown_rx) = mpsc::channel();

        // Define system service event handler that will be receiving service events.
        let event_handler = move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                // Notifies a service to report its current status information to the service
                // control manager. Always return NoError even if not implemented.
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,

                // Handle stop
                ServiceControl::Stop => {
                    shutdown_tx.send(()).unwrap();
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
        rt.block_on(super::realmain());

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
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        Ok(())
    }
}

#[cfg(not(windows))]
#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .filter_module("rustls", log::LevelFilter::Error) // This baby gets noisy at lower levels
        .init();

    realmain().await;
}

async fn _test_poop() {
    loop {
        tokio::time::delay_for(Duration::from_secs(3)).await;
        warn!("poop");
        error!("pooerrp");
        info!("poopinf");
    }
}

async fn realmain() {
    // TODO: see if there is an override so if this fails we are still ok?
    let roots = match load_roots() {
        Some(r) => r,
        None => {
            eprintln!("Error loading trusted certificates");
            error!("Error loading trusted certificates");
            std::process::exit(1);
        }
    };

    let notify = Arc::new(Notify::new());
    let notifyc = notify.clone();
    let mut watcher: RecommendedWatcher =
        Watcher::new_immediate(move |res: Result<Event, notify::Error>| {
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
        })
        .expect("Unable to create file event listener");

    // We only care about mutations
    if let Err(e) = watcher.configure(Config::PreciseEvents(true)) {
        eprintln!("Unable to set file event configuration options (precise) {:?}", e);
        error!("Unable to set file event configuration options (precise) {:?}", e);
        std::process::exit(1);
    }

    // Add a path to be watched. All files and directories at that path and
    // below will be monitored for changes.
    if let Err(e) = watcher.watch(ebbflow::CONFIG_PATH, RecursiveMode::Recursive) {
        eprintln!("Unable to set file event configuration options {:?}", e);
        error!("Unable to set file event configuration options {:?}", e);
        std::process::exit(1);
    }

    let sharedinfo = Arc::new(
        SharedInfo::new_with_ebbflow_overrides(
            "127.0.0.1:7070".parse().unwrap(),
            "s.preview.ebbflow.io".to_string(),
            roots,
        )
        .await
        .unwrap(),
    );

    let runner = run_daemon(sharedinfo, Box::pin(config_reload), load_roots, notify).await;

    tokio::spawn(async move {
        loop {
            tokio::time::delay_for(Duration::from_secs(60 * 5)).await;
            info!("Status\n{:#?}", runner.status().await);
        }
    });

    futures::future::pending::<()>().await;
}

pub fn config_reload() -> BoxFuture<'static, Result<EbbflowDaemonConfig, ConfigError>> {
    Box::pin(async { EbbflowDaemonConfig::load_from_file().await })
}

pub fn load_roots() -> Option<RootCertStore> {
    match rustls_native_certs::load_native_certs() {
        rustls_native_certs::PartialResult::Ok(rcs) => Some(rcs),
        rustls_native_certs::PartialResult::Err((Some(rcs), _)) => Some(rcs),
        _ => None,
    }
}
