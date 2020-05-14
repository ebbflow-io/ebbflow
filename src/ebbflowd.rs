#[macro_use] extern crate log;

use ebbflow::daemon::{spawn_endpoint, EndpointArgs, SharedInfo,};
use ebbflow::daemon::connection::EndpointConnectionType;
use ebbflow::config::{Endpoint, ConfigError, EbbflowDaemonConfig};
use ebbflow::signal::SignalSender;
use std::sync::Arc;
use std::net::{Ipv4Addr, SocketAddrV4};
use rustls::RootCertStore;
use futures::future::BoxFuture;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::Entry;
use std::time::Duration;

const MAX_MAX_IDLE: usize = 100;
const DEFAULT_MAX_IDLE: usize = 8;
const LOAD_CFG_TIMEOUT: Duration = Duration::from_secs(3);
const LOAD_CONFIG_DELAY: Duration = Duration::from_secs(60);

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

struct DaemonRunner {
    endpoints: HashMap<String, EndpointInstance>,
    info: Arc<SharedInfo>,
}

struct EndpointInstance {
    stop_sender: SignalSender,
    existing_config: Endpoint,
}

impl DaemonRunner {
    pub fn new(info: Arc<SharedInfo>) -> Self {
        Self {
            info,
            endpoints: HashMap::new(),
        }
    }

    pub fn update_config(&mut self, mut config: EbbflowDaemonConfig) {
        let mut set = HashSet::with_capacity(config.endpoints.len());
        for e in config.endpoints.iter() {
            set.insert(e.dns.clone());
        }

        self.endpoints.retain(|existing_dns, existing_instance| {
            if set.contains(existing_dns) {
                trace!("set compare had existing {}", existing_dns);
                true
            } else {
                debug!("set compare no longer had existing entry {}", existing_dns);
                // The set of configs does NOT have this endpoint, we need to stop it.
                existing_instance.stop_sender.send_signal();
                // It will stop, new we return false to state we should remove this entry.
                false
            }
        });

        for endpoint in config.endpoints.drain(..) {
            match self.endpoints.entry(endpoint.dns.clone()) {
                Entry::Occupied(mut oe) => {
                    // if the same, do nothing
                    let current_instance = oe.get();
                    
                    if current_instance.existing_config == endpoint {
                        trace!("Configuration for an endpoint did not change, doing nothing {}", endpoint.dns);
                        // do nothing!!s
                    } else {
                        debug!("Configuration for an endpoint CHANGED, will stop existing and start new one {}", endpoint.dns);
                        // Stop the existing one
                        current_instance.stop_sender.send_signal();
                        
                        // Create a new one
                        let sender = spawn_endpointasdfsfa(endpoint.clone(), self.info.clone());

                        // Insert this new config
                        oe.insert(EndpointInstance {
                            stop_sender: sender,
                            existing_config: endpoint,
                        });
                    }
                }
                Entry::Vacant(ve) => {
                    debug!("Configuration for an endpoint that did NOT previously exist found, will create it {}", endpoint.dns);
                    let sender = spawn_endpointasdfsfa(endpoint.clone(), self.info.clone());
                    ve.insert(EndpointInstance {
                        stop_sender: sender,
                        existing_config: endpoint,
                    });
                }
            }
        }        
    }

    pub fn update_roots(&self, roots: RootCertStore) {
        self.info.update_roots(roots);
    }
}

pub fn spawn_endpointasdfsfa(e: ebbflow::config::Endpoint, info: Arc<SharedInfo>) -> SignalSender {
    let address = e.address_override.unwrap_or_else(|| "127.0.0.1".to_string());

    let ip = address.parse().unwrap();

    let port = e.port;

    let idle = e.idleconns_override.unwrap_or(DEFAULT_MAX_IDLE);
    let idle = std::cmp::min(idle, MAX_MAX_IDLE);

    let args = EndpointArgs {
        ctype: EndpointConnectionType::Tls,
        idleconns: idle,
        maxconns: e.maxconns as usize,
        endpoint: e.dns,
        local_addr: SocketAddrV4::new(ip, port),
    };

    let sender = SignalSender::new();
    let receiver = sender.new_receiver();

    tokio::spawn(async move {
        let _ = spawn_endpoint(info, args, receiver).await;
    });

    sender
}

pub async fn run_daemon<CFGR, ROOTR>(initial_config: EbbflowDaemonConfig, info: Arc<SharedInfo>, cfg_reload: CFGR, root_reload: ROOTR)
where
    CFGR: Fn() -> BoxFuture<'static, Result<EbbflowDaemonConfig, ConfigError>>,
    ROOTR: Fn() -> Option<RootCertStore>,
{
    let mut runner = DaemonRunner::new(info);
    runner.update_config(initial_config);

    loop {
        tokio::time::delay_for(LOAD_CONFIG_DELAY).await;
        debug!("Reloading config and roots");
        if let Ok(Ok(cfg)) = tokio::time::timeout(LOAD_CFG_TIMEOUT, cfg_reload()).await {
            runner.update_config(cfg);
        }

        if let Some(newroots) = root_reload() {
            runner.update_roots(newroots);
        }
    }
}

pub fn load_roots() -> Option<RootCertStore> {
    match rustls_native_certs::load_native_certs() {
       rustls_native_certs::PartialResult::Ok(rcs) => Some(rcs),
       rustls_native_certs::PartialResult::Err((Some(rcs), _)) => Some(rcs),
       _ => None
    }
}

// Constantly load configuration
// Based on configuration, spawn things
// Hold onto pointer to things so we can change based on configuration

