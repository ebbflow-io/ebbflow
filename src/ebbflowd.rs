use ebbflow::config::{ConfigError, EbbflowDaemonConfig};
use ebbflow::daemon::SharedInfo;
use ebbflow::run_daemon;
use futures::future::BoxFuture;
use rustls::RootCertStore;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .filter_module("rustls", log::LevelFilter::Error) // This baby gets noisy at lower levels
        .init();

    let hostname: String = hostname::get().unwrap().to_str().unwrap().to_string();

    let config = EbbflowDaemonConfig::load_from_file().await.unwrap();
    let roots = load_roots().unwrap();
    //let sharedinfo = Arc::new(SharedInfo::new(config.key.clone(), roots).await.unwrap());
    let sharedinfo = Arc::new(SharedInfo::new_with_ebbflow_overrides("127.0.0.1:7070".parse().unwrap(),config.key.clone(), roots, hostname).await.unwrap());

    run_daemon(config, sharedinfo, config_reload, load_roots).await;
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
