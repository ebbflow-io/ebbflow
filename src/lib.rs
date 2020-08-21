#[macro_use]
extern crate log;
#[macro_use]
extern crate lazy_static;

use crate::config::{ConfigError, EbbflowDaemonConfig, Endpoint};
use crate::daemon::connection::EndpointConnectionType;
use crate::daemon::EndpointMeta;
use crate::daemon::{spawn_endpoint, EndpointArgs, SharedInfo};
use daemon::HealthOverall;
use futures::future::BoxFuture;
use messagequeue::MessageQueue;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::Notify;

pub const MAX_MAX_IDLE: usize = 1000;

pub mod certs;
pub mod config;
pub mod daemon;
pub mod dns;
pub mod infoserver;
pub mod messagequeue;
pub mod messaging;
pub mod signal;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DaemonStatusMeta {
    Uninitialized,
    Good,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub meta: DaemonStatusMeta,
    pub endpoints: Vec<(String, DaemonEndpointStatus)>,
    pub ssh: Option<(String, DaemonEndpointStatus)>,
    pub messages: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DaemonEndpointStatus {
    Disabled,
    Enabled {
        active: usize,
        idle: usize,
        health: Option<HealthOverall>,
    },
}

impl DaemonEndpointStatus {
    fn from_ref(ed: &EnabledDisabled) -> Self {
        match ed {
            EnabledDisabled::Disabled => DaemonEndpointStatus::Disabled,
            EnabledDisabled::Enabled(meta) => DaemonEndpointStatus::Enabled {
                active: meta.num_active(),
                idle: meta.num_idle(),
                health: Some(meta.health()),
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
    maxidle: usize,
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

    pub async fn update_config(&self, config: Option<EbbflowDaemonConfig>, key: Option<String>) {
        let mut inner = self.inner.lock().await;
        inner.update_config(config, key).await;
    }

    pub async fn status(&self) -> DaemonStatus {
        let inner = self.inner.lock().await;
        inner.status()
    }

    pub async fn submit_error_message(&self, message: String) {
        self.inner.lock().await.submit_error_message(message);
    }
}

struct InnerDaemonRunner {
    endpoints: HashMap<String, EndpointInstance>,
    statusmeta: DaemonStatusMeta,
    ssh: Option<SshInstance>,
    info: Arc<SharedInfo>,
    message_queue: Arc<MessageQueue>,
}

impl InnerDaemonRunner {
    pub fn new(info: Arc<SharedInfo>) -> Self {
        Self {
            endpoints: HashMap::new(),
            ssh: None,
            statusmeta: DaemonStatusMeta::Uninitialized,
            info,
            message_queue: Arc::new(MessageQueue::new()),
        }
    }

    pub fn submit_error_message(&self, message: String) {
        self.message_queue.add_message(message);
    }

    pub async fn update_config(
        &mut self,
        config: Option<EbbflowDaemonConfig>,
        key: Option<String>,
    ) {
        // Update the key
        if let Some(k) = key {
            self.info.update_key(k);
        }

        // We do this so we can later info.key().unwrap(). (defense in depth)
        if self.info.key().is_none() {
            self.statusmeta = DaemonStatusMeta::Uninitialized;
            let e = format!(
                "INFO: Key not set, doing nothing (cfg present {})",
                config.is_some()
            );
            self.submit_error_message(e.to_string());
            error!("{}", e);
            return;
        }
        self.statusmeta = DaemonStatusMeta::Good;

        let mut config = match config {
            Some(c) => c,
            None => return,
        };

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
                        debug!(
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
                            let meta = spawn_endpointasdfsfa(
                                endpoint.clone(),
                                self.info.clone(),
                                self.message_queue.clone(),
                            )
                            .await;
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
                    let enabledisable = if endpoint.enabled {
                        EnabledDisabled::Enabled(
                            spawn_endpointasdfsfa(
                                endpoint.clone(),
                                self.info.clone(),
                                self.message_queue.clone(),
                            )
                            .await,
                        )
                    } else {
                        EnabledDisabled::Disabled
                    };
                    ve.insert(EndpointInstance {
                        enabledisable,
                        existing_config: endpoint,
                    });
                }
            }
        }

        match (config.ssh, &mut self.ssh) {
            (Some(newcfg), ssh) => {
                let newconfig = SshConfiguration {
                    port: newcfg.port,
                    max: newcfg.maxconns as usize,
                    hostname: newcfg.hostname_override.unwrap_or_else(hostname_or_die),
                    enabled: newcfg.enabled,
                    maxidle: newcfg.maxidle as usize,
                };

                if ssh.is_none() || newconfig != ssh.as_ref().unwrap().existing_config {
                    if let Some(instance) = ssh {
                        instance.enabledisable.stop();
                    }
                    let enabledisabled = if newconfig.enabled {
                        // start the new one and set it
                        let args = EndpointArgs {
                            ctype: EndpointConnectionType::Ssh,
                            idleconns: newconfig.maxidle,
                            maxconns: newconfig.max,
                            endpoint: newconfig.hostname.clone(),
                            port: newconfig.port,
                            message_queue: self.message_queue.clone(),
                            healthcheck: None,
                        };

                        let meta = spawn_endpoint(self.info.clone(), args).await;
                        EnabledDisabled::Enabled(meta)
                    } else {
                        EnabledDisabled::Disabled
                    };

                    self.ssh = Some(SshInstance {
                        existing_config: newconfig,
                        enabledisable: enabledisabled,
                    });
                }
                // else they are equal so do nothing
            }
            (None, Some(oldcfg)) => {
                oldcfg.enabledisable.stop();
                self.ssh = None;
            }
            (None, None) => {}
        }
    }

    pub fn status(&self) -> DaemonStatus {
        let ssh = match &self.ssh {
            Some(sshinstance) => Some((
                sshinstance.existing_config.hostname.clone(),
                DaemonEndpointStatus::from_ref(&sshinstance.enabledisable),
            )),
            None => None,
        };
        let e = self
            .endpoints
            .iter()
            .map(|(s, e)| (s.clone(), DaemonEndpointStatus::from_ref(&e.enabledisable)))
            .collect();

        DaemonStatus {
            meta: self.statusmeta,
            endpoints: e,
            ssh,
            messages: self.message_queue.get_messages(),
        }
    }
}

pub async fn spawn_endpointasdfsfa(
    e: crate::config::Endpoint,
    info: Arc<SharedInfo>,
    message_queue: Arc<MessageQueue>,
) -> Arc<EndpointMeta> {
    let port = e.port;

    let idle = e.maxidle;
    let idle = std::cmp::min(idle, MAX_MAX_IDLE as u16);

    let args = EndpointArgs {
        ctype: EndpointConnectionType::Tls,
        idleconns: idle as usize,
        maxconns: e.maxconns as usize,
        endpoint: e.dns,
        port,
        message_queue,
        healthcheck: e.healthcheck,
    };

    spawn_endpoint(info, args).await
}

#[allow(clippy::type_complexity)]
pub async fn run_daemon(
    info: Arc<SharedInfo>,
    cfg_reload: Pin<
        Box<
            dyn Fn() -> BoxFuture<
                    'static,
                    Result<(Option<EbbflowDaemonConfig>, Option<String>), ConfigError>,
                > + Send
                + Sync
                + 'static,
        >,
    >,
    cfg_notifier: Arc<Notify>,
) -> Arc<DaemonRunner> {
    let runner = Arc::new(DaemonRunner::new(info));
    let runnerc = runner.clone();

    let cfgrealoadfn = cfg_reload;

    tokio::spawn(async move {
        loop {
            match cfgrealoadfn().await {
                Ok((newconfig, newkey)) => {
                    debug!("New config loaded successfully");
                    runnerc.update_config(newconfig, newkey).await;
                    debug!("New config applied");
                }
                Err(e) => {
                    let e = format!("Error reading new configuration {:?}", e);
                    warn!("{}", e);
                    runnerc.submit_error_message(e).await;
                }
            }
            trace!("Now waiting for notification");
            cfg_notifier.notified().await;
            trace!("Got a notification");
        }
    });

    runner
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
