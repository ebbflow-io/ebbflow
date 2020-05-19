#[macro_use]
extern crate log;
extern crate env_logger;
use ebbflow::config::{ConfigError, EbbflowDaemonConfig, Endpoint, Ssh};

#[tokio::main]
async fn main() {
    println!("Hey, not sleeping done now bye");
}

// Uses AccountId, creates a key, then prompts for endpoint/port combos and if SSH should be enabled
async fn init() {}

// init
// status (endpoints and ssh)
// get config file loc

async fn finish_init(
    key: String,
    enable_ssh: bool,
    mut endpoints: Vec<(String, u16)>,
) -> Result<(), ConfigError> {
    let endpoints = endpoints
        .drain(..)
        .map(|(dns, port)| Endpoint {
            port,
            dns,
            maxconns: 200,
            idleconns_override: None,
            address_override: None,
            enabled: true,
        })
        .collect();

    // create the initial config
    let cfg = EbbflowDaemonConfig {
        key,
        endpoints,
        ssh: Ssh::new(enable_ssh),
    };

    cfg.save_to_file().await
}

async fn set_endpoint_enabled(enabled: bool, dns: Option<&str>) -> Result<(), ConfigError> {
    let mut existing = EbbflowDaemonConfig::load_from_file().await?;

    for e in existing.endpoints.iter_mut() {
        if let Some(actualdns) = dns {
            if actualdns == &e.dns {
                e.enabled = enabled;
            }
        } else {
            e.enabled = enabled;
        }
    }

    existing.save_to_file().await?;

    Ok(())
}

async fn set_ssh_enabled(enabled: bool) -> Result<(), ConfigError> {
    let mut existing = EbbflowDaemonConfig::load_from_file().await?;
    existing.ssh.enabled = enabled;
    existing.save_to_file().await?;
    Ok(())
}

async fn set_all_enabled(enabled: bool) -> Result<(), ConfigError> {
    set_endpoint_enabled(enabled, None).await?;
    set_ssh_enabled(enabled).await?;
    Ok(())
}
