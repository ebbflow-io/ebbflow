use ebbflow::daemon::SharedInfo;
use ebbflow::config::{ConfigError, EbbflowDaemonConfig};
use std::sync::Arc;
use rustls::RootCertStore;
use futures::future::BoxFuture;
use ebbflow::run_daemon;

#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .init();

    let config = EbbflowDaemonConfig::load_from_file().await.unwrap();
    let roots = load_roots().unwrap();
    let sharedinfo = Arc::new(SharedInfo::new(config.key.clone(), roots).await.unwrap());

    run_daemon(config, sharedinfo, config_reload, load_roots).await;
}

pub fn config_reload() -> BoxFuture<'static, Result<EbbflowDaemonConfig, ConfigError>> {
    Box::pin(async {
        EbbflowDaemonConfig::load_from_file().await
    })
}

pub fn load_roots() -> Option<RootCertStore> {
    match rustls_native_certs::load_native_certs() {
       rustls_native_certs::PartialResult::Ok(rcs) => Some(rcs),
       rustls_native_certs::PartialResult::Err((Some(rcs), _)) => Some(rcs),
       _ => None
    }
}