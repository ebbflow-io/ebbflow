#[macro_use]
extern crate log;

use crate::config::{ConfigError, EbbflowDaemonConfig, Endpoint};
use crate::daemon::connection::EndpointConnectionType;
use crate::daemon::{spawn_endpoint, EndpointArgs, SharedInfo};
use crate::signal::SignalSender;
use futures::future::BoxFuture;
use rustls::RootCertStore;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::collections::HashSet;
use std::net::SocketAddrV4;
use std::sync::Arc;
use std::time::Duration;
use std::net::Ipv4Addr;

const MAX_MAX_IDLE: usize = 100;
const DEFAULT_MAX_IDLE: usize = 8;
const DEFAULT_MAX_IDLE_SSH: usize = 2;
const LOAD_CFG_TIMEOUT: Duration = Duration::from_secs(3);
const LOAD_CONFIG_DELAY: Duration = Duration::from_secs(60);

pub mod config;
pub mod daemon;
pub mod dns;
pub mod messaging;
pub mod signal;

struct DaemonRunner {
    endpoints: HashMap<String, EndpointInstance>,
    /// port max hostname
    ssh: Option<(SshConfiguration, SignalSender)>,
    info: Arc<SharedInfo>,
}

struct EndpointInstance {
    stop_sender: SignalSender,
    existing_config: Endpoint,
}

#[derive(PartialEq)]
struct SshConfiguration {
    port: u16,
    max: usize,
    hostname: String,
    key: String,
}

impl DaemonRunner {
    pub fn new(info: Arc<SharedInfo>) -> Self {
        Self {
            info,
            endpoints: HashMap::new(),
            ssh: None,
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
                        trace!(
                            "Configuration for an endpoint did not change, doing nothing {}",
                            endpoint.dns
                        );
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

        if config.enable_ssh {
            let port = config.ssh.as_ref().map(|sshcfg| sshcfg.port).unwrap_or(22);
            let max: usize = config.ssh.as_ref().map(|sshcfg| sshcfg.maxconns as usize).unwrap_or(30);
            let hostname: Option<String> = config.ssh.as_ref().map(|sshcfg| sshcfg.hostname_override.clone()).flatten();
            let hostname = hostname.unwrap_or_else(|| self.info.hostname());
            let newkey = config.key.clone();

            let newconfig = SshConfiguration {
                key: newkey,
                port,
                max,
                hostname,
            };

            // if its the same, do nothing
            if if let Some((cfg, s)) = &self.ssh {
                if cfg != &newconfig {
                    s.send_signal();
                    true
                } else {
                    false
                }
            } else {
                self.ssh.is_none()
            } {
                // start the new one and set it
                let args = EndpointArgs {
                    ctype: EndpointConnectionType::Ssh,
                    idleconns: DEFAULT_MAX_IDLE_SSH,
                    maxconns: newconfig.max,
                    endpoint: newconfig.hostname.clone(),
                    local_addr: SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), newconfig.port),
                };

                let sender = spawn_endpoint_with_args(args, self.info.clone());
                self.ssh = Some((newconfig, sender));
            }
            

        }
    }

    pub fn update_roots(&self, roots: RootCertStore) {
        self.info.update_roots(roots);
    }
}

pub fn spawn_endpointasdfsfa(e: crate::config::Endpoint, info: Arc<SharedInfo>) -> SignalSender {
    let address = e
        .address_override
        .unwrap_or_else(|| "127.0.0.1".to_string());

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

    spawn_endpoint_with_args(args, info)
}

pub fn spawn_endpoint_with_args(args: EndpointArgs, info: Arc<SharedInfo>) -> SignalSender {
    let sender = SignalSender::new();
    let receiver = sender.new_receiver();

    tokio::spawn(async move {
        let _ = spawn_endpoint(info, args, receiver).await;
    });

    sender
}

pub async fn run_daemon<CFGR, ROOTR>(
    initial_config: EbbflowDaemonConfig,
    info: Arc<SharedInfo>,
    cfg_reload: CFGR,
    root_reload: ROOTR,
) where
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
