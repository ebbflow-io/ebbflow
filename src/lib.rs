#[macro_use]
extern crate log;

use crate::config::{ConfigError, EbbflowDaemonConfig, Endpoint};
use crate::daemon::connection::EndpointConnectionType;
use crate::daemon::EndpointMeta;
use crate::daemon::{spawn_endpoint, EndpointArgs, SharedInfo};
use futures::future::BoxFuture;
use rustls::RootCertStore;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::collections::HashSet;
use std::net::SocketAddrV4;
use std::sync::Arc;
use std::time::Duration;
use std::{net::Ipv4Addr, pin::Pin};
use tokio::sync::Mutex;
use tokio::sync::Notify;

/// Path to the Config file, see EbbflowDaemonConfig in the config module.
pub const CONFIG_PATH: &str = "/etc/ebbflow/"; // Linux
pub const CONFIG_FILE: &str = "/etc/ebbflow/config.yaml"; // Linux

const MAX_MAX_IDLE: usize = 100;
const DEFAULT_MAX_IDLE: usize = 8;
const DEFAULT_MAX_IDLE_SSH: usize = 2;
// const LOAD_CFG_TIMEOUT: Duration = Duration::from_secs(3);
const LOAD_ROOTS_DELAY: Duration = Duration::from_secs(60 * 60 * 36); // very rare

pub mod config;
pub mod daemon;
pub mod dns;
pub mod messaging;
pub mod signal;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DaemonStatusMeta {
    Uninitialized,
    Good,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DaemonStatus {
    pub meta: DaemonStatusMeta,
    pub endpoints: Vec<(String, DaemonEndpointStatus)>,
    pub ssh: DaemonEndpointStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DaemonEndpointStatus {
    Disabled,
    Enabled { active: usize, idle: usize },
}

impl DaemonEndpointStatus {
    fn from_ref(ed: &EnabledDisabled) -> Self {
        match ed {
            EnabledDisabled::Disabled => DaemonEndpointStatus::Disabled,
            EnabledDisabled::Enabled(meta) => DaemonEndpointStatus::Enabled {
                active: meta.num_active(),
                idle: meta.num_idle(),
            },
        }
    }
}

/// terribly named but its getting tough to think of words
enum EnabledDisabled {
    Enabled(Arc<EndpointMeta>),
    Disabled,
}

impl EnabledDisabled {
    pub fn stop(&mut self) {
        if let EnabledDisabled::Enabled(meta) = self {
            debug!("Sending signal to stop");
            meta.stop();
        }
        *self = EnabledDisabled::Disabled;
    }
}

struct EndpointInstance {
    enabledisable: EnabledDisabled,
    existing_config: Endpoint,
}

struct SshInstance {
    existing_config: SshConfiguration,
    enabledisable: EnabledDisabled,
}

#[derive(Debug, PartialEq)]
struct SshConfiguration {
    port: u16,
    max: usize,
    hostname: String,
    enabled: bool,
}

pub enum EnableDisableTarget {
    All,
    Ssh,
    Endpoint(String),
}

pub struct DaemonRunner {
    inner: Mutex<InnerDaemonRunner>,
}

impl DaemonRunner {
    pub fn new(info: Arc<SharedInfo>) -> Self {
        Self {
            inner: Mutex::new(InnerDaemonRunner::new(info)),
        }
    }

    pub async fn update_config(&self, config: EbbflowDaemonConfig) {
        let mut inner = self.inner.lock().await;
        inner.update_config(config).await;
    }

    pub async fn update_roots(&self, roots: RootCertStore) {
        let inner = self.inner.lock().await;
        inner.update_roots(roots);
    }

    pub async fn status(&self) -> DaemonStatus {
        let inner = self.inner.lock().await;
        inner.status()
    }
}

struct InnerDaemonRunner {
    endpoints: HashMap<String, EndpointInstance>,
    statusmeta: DaemonStatusMeta,
    ssh: SshInstance,
    info: Arc<SharedInfo>,
}

impl InnerDaemonRunner {
    pub fn new(info: Arc<SharedInfo>) -> Self {
        Self {
            endpoints: HashMap::new(),
            ssh: SshInstance {
                enabledisable: EnabledDisabled::Disabled,
                existing_config: defaultsshconfig(info.hostname()),
            },
            statusmeta: DaemonStatusMeta::Uninitialized,
            info,
        }
    }

    pub async fn update_config(&mut self, mut config: EbbflowDaemonConfig) {
        self.info.update_key(config.key);

        // We do this so we can later info.key().unwrap().
        if self.info.key().is_none() {
            error!("ERROR: Unreachable state where we do not have a key to use, but do have an otherwise valid configuration");
            return;
        }
        self.statusmeta = DaemonStatusMeta::Good;

        let mut set = HashSet::with_capacity(config.endpoints.len());
        trace!("Config updating, {} endpoints", config.endpoints.len());
        for e in config.endpoints.iter() {
            debug!("reading config endpoint {} enabled {}", e.dns, e.enabled);
            set.insert(e.dns.clone());
        }

        self.endpoints.retain(|existing_dns, existing_instance| {
            if set.contains(existing_dns) {
                trace!("set compare had existing {}", existing_dns);
                true
            } else {
                debug!("set compare no longer had existing entry {}", existing_dns);
                // The set of configs does NOT have this endpoint, we need to stop it.
                existing_instance.enabledisable.stop();
                // It will stop, new we return false to state we should remove this entry.
                false
            }
        });

        for endpoint in config.endpoints.drain(..) {
            match self.endpoints.entry(endpoint.dns.clone()) {
                Entry::Occupied(mut oe) => {
                    // if the same, do nothing
                    let current_instance = oe.get_mut();

                    if current_instance.existing_config == endpoint {
                        trace!(
                            "Configuration for an endpoint did not change, doing nothing {}",
                            endpoint.dns
                        );
                    // do nothing!!s
                    } else {
                        debug!("Configuration for an endpoint CHANGED, will stop existing and start new one {}", endpoint.dns);

                        let newenabledisable = if endpoint.enabled {
                            debug!("New configuration is enabled, stopping existing one and setting new one to enabled");
                            // Stop the existing one (may not be running anways)
                            current_instance.enabledisable.stop();
                            // Create a new one
                            let meta =
                                spawn_endpointasdfsfa(endpoint.clone(), self.info.clone()).await;
                            EnabledDisabled::Enabled(meta)
                        } else {
                            debug!("New configuration is DISABLED, stopping existing one and setting new one to enabled");
                            // stop the current one. If it wasn't running anways, then this is still OK.
                            current_instance.enabledisable.stop();
                            // we weren't running, so just return disabled
                            EnabledDisabled::Disabled
                        };

                        // Insert this new config
                        oe.insert(EndpointInstance {
                            enabledisable: newenabledisable,
                            existing_config: endpoint,
                        });
                    }
                }
                Entry::Vacant(ve) => {
                    debug!("Configuration for an endpoint that did NOT previously exist found, will create it {}", endpoint.dns);
                    let sender = spawn_endpointasdfsfa(endpoint.clone(), self.info.clone()).await;
                    ve.insert(EndpointInstance {
                        enabledisable: EnabledDisabled::Enabled(sender),
                        existing_config: endpoint,
                    });
                }
            }
        }

        // SSH Related
        let hostname: Option<String> = config.ssh.hostname_override.clone();
        let hostname = hostname.unwrap_or_else(|| self.info.hostname());

        let newconfig = SshConfiguration {
            port: config.ssh.port,
            max: config.ssh.maxconns as usize,
            hostname,
            enabled: config.ssh.enabled,
        };

        // If something changed, we know we will stop the existing one
        if newconfig != self.ssh.existing_config {
            trace!("Old config\n{:#?}", self.ssh.existing_config);
            trace!("New config\n{:#?}", newconfig);
            self.ssh.enabledisable.stop();

            //We have a different config, and its new, lets start the new one.
            if newconfig.enabled {
                // start the new one and set it
                let args = EndpointArgs {
                    ctype: EndpointConnectionType::Ssh,
                    idleconns: DEFAULT_MAX_IDLE_SSH,
                    maxconns: newconfig.max,
                    endpoint: newconfig.hostname.clone(),
                    local_addr: SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), newconfig.port),
                };

                let meta = spawn_endpoint(self.info.clone(), args).await;
                self.ssh.enabledisable = EnabledDisabled::Enabled(meta);
            }
        }

        // Either its new, and we want to set it to the new one, or its the same, and this is OK.
        self.ssh.existing_config = newconfig;
    }

    pub fn update_roots(&self, roots: RootCertStore) {
        self.info.update_roots(roots);
    }

    pub fn status(&self) -> DaemonStatus {
        let ssh = DaemonEndpointStatus::from_ref(&self.ssh.enabledisable);

        let e = self
            .endpoints
            .iter()
            .map(|(s, e)| (s.clone(), DaemonEndpointStatus::from_ref(&e.enabledisable)))
            .collect();

        DaemonStatus {
            meta: self.statusmeta,
            endpoints: e,
            ssh,
        }
    }
}

fn defaultsshconfig(hostname: String) -> SshConfiguration {
    SshConfiguration {
        hostname,
        port: 22,
        max: 20,
        enabled: false,
    }
}

pub async fn spawn_endpointasdfsfa(
    e: crate::config::Endpoint,
    info: Arc<SharedInfo>,
) -> Arc<EndpointMeta> {
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

    spawn_endpoint(info, args).await
}

pub async fn run_daemon<ROOTR>(
    info: Arc<SharedInfo>,
    cfg_reload: Pin<
        Box<
            dyn Fn() -> BoxFuture<'static, Result<EbbflowDaemonConfig, ConfigError>>
                + Send
                + Sync
                + 'static,
        >,
    >,
    root_reload: ROOTR,
    cfg_notifier: Arc<Notify>,
) -> Arc<DaemonRunner>
where
    //CFGR: Pin<Box<dyn Fn() -> BoxFuture<'static, Result<EbbflowDaemonConfig, ConfigError>>>>,
    ROOTR: Fn() -> Option<RootCertStore> + Sync + Send + 'static,
{
    let runner = Arc::new(DaemonRunner::new(info));
    let runnerc = runner.clone();

    let cfgrealoadfn = cfg_reload;

    tokio::spawn(async move {
        loop {
            match cfgrealoadfn().await {
                Ok(newconfig) => {
                    debug!("New config loaded successfully");
                    runnerc.update_config(newconfig).await;
                    debug!("New config applied");
                }
                Err(e) => {
                    warn!("Error reading new configuration {:?}", e);
                }
            }
            trace!("Now waiting for notification");
            cfg_notifier.notified().await;
            trace!("Got a notification");
        }
    });
    let runnerc = runner.clone();

    tokio::spawn(Box::pin(async move {
        loop {
            tokio::time::delay_for(LOAD_ROOTS_DELAY).await;
            if let Some(newroots) = root_reload() {
                runner.update_roots(newroots).await;
            } else {
                warn!("Was unable to load new root certificates from OS, will continue to use existing set.");
            }
        }
    }));

    runnerc
}

pub fn hostname_or_die() -> String {
    match hostname::get() {
        Ok(s) => {
            match s.to_str() {
                Some(s) => s.to_string(),
                None => {
                    eprintln!("Error retrieving the hostname from the OS, could not turn {:?} into String", s);
                    error!("Error retrieving the hostname from the OS, could not turn {:?} into String", s);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("Error retrieving the hostname from the OS {:?}", e);
            error!("Error retrieving the hostname from the OS {:?}", e);
            std::process::exit(1);
        }
    }
}
