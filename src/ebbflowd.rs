#[macro_use]
extern crate log;

use ebbflow::config::{ConfigError, EbbflowDaemonConfig};
use ebbflow::daemon::SharedInfo;
use ebbflow::run_daemon;
use ebbflow::hostname_or_die;
use futures::future::BoxFuture;
use notify::{event::Event, event::EventKind, Config, RecommendedWatcher, RecursiveMode, Watcher};
use rustls::RootCertStore;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .filter_module("rustls", log::LevelFilter::Error) // This baby gets noisy at lower levels
        .init();

    // TODO: see if there is an override so if this fails we are still ok?
    let hostname: String = hostname_or_die();
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
        eprintln!("Unable to set file event configuration options {:?}", e);
        error!("Unable to set file event configuration options {:?}", e);
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
            hostname,
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
