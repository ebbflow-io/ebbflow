use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::{fs::OpenOptions, net::SocketAddrV4};
use tokio::fs;
use tokio::fs::OpenOptions as TokioOpenOptions;
use tokio::io::Error as IoError;
use tokio::io::ErrorKind;

// Path to the Config file, see EbbflowDaemonConfig in the config module.
#[cfg(target_os = "linux")]
lazy_static! {
    pub static ref CONFIG_PATH: String = {
        match std::env::var("EBB_CFG_DIR").ok() {
            Some(p) => p.trim_end_matches('/').to_string(),
            None => "/etc/ebbflow".to_string(),
        }
    };
}
#[cfg(target_os = "macos")]
pub const CONFIG_PATH: &str = "/usr/local/etc/ebbflow";
#[cfg(windows)]
lazy_static! {
    pub static ref CONFIG_PATH: String = { "\\Program Files\\ebbflow".to_string() };
}

pub fn config_path_root() -> String {
    CONFIG_PATH.to_string()
}

#[cfg(windows)]
pub fn config_file_full() -> String {
    format!("{}\\{}", config_path_root(), CONFIG_FILE)
}

#[cfg(not(windows))]
pub fn config_file_full() -> String {
    format!("{}/{}", config_path_root(), CONFIG_FILE)
}

#[cfg(windows)]
pub fn key_file_full() -> String {
    format!("{}\\{}", config_path_root(), KEY_FILE)
}

#[cfg(not(windows))]
pub fn key_file_full() -> String {
    format!("{}/{}", config_path_root(), KEY_FILE)
}

#[cfg(windows)]
pub fn addr_file_full() -> String {
    format!("{}\\{}", config_path_root(), ADDR_FILE)
}

#[cfg(not(windows))]
pub fn addr_file_full() -> String {
    format!("{}/{}", config_path_root(), ADDR_FILE)
}

pub async fn write_addr(addr: &str) -> Result<(), ConfigError> {
    Ok(fs::write(addr_file_full(), addr.trim()).await?)
}

pub async fn read_addr() -> Result<SocketAddrV4, ConfigError> {
    let s = fs::read_to_string(addr_file_full()).await?;
    if s.is_empty() {
        return Err(ConfigError::Empty);
    }

    Ok(s.trim()
        .to_string()
        .parse()
        .map_err(|_| ConfigError::Parsing)?)
}

pub const CONFIG_FILE: &str = "config.yaml";
pub const KEY_FILE: &str = "host.key";
pub const ADDR_FILE: &str = ".daemonaddr";

#[derive(PartialEq, Debug)]
pub enum ConfigError {
    Parsing,
    FileNotFound,
    FilePermissions,
    Empty,
    Unknown(String),
}

pub async fn getkey() -> Result<String, ConfigError> {
    let s = fs::read_to_string(key_file_full()).await?;
    if s.is_empty() {
        return Err(ConfigError::Empty);
    }

    Ok(s.trim().to_string())
}

pub async fn setkey(k: &str) -> Result<(), ConfigError> {
    Ok(fs::write(key_file_full(), k.trim().as_bytes()).await?)
}

impl From<IoError> for ConfigError {
    fn from(ioe: IoError) -> Self {
        match ioe.kind() {
            ErrorKind::NotFound => ConfigError::FileNotFound,
            ErrorKind::PermissionDenied => ConfigError::FilePermissions,
            _ => ConfigError::Unknown(format!("Unexepected error reading config file {:?}", ioe)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PossiblyEmptyEbbflowDaemonConfig {
    Empty,
    EbbflowDaemonConfig(EbbflowDaemonConfig),
}

/// Configuration for Ebbflow. Will be parsed to/from a YAML file located at
/// - /etc/ebbflow for Linux
/// - TBD for Windows
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbbflowDaemonConfig {
    /// A list of endpoints to host, see Endpoint
    pub endpoints: Vec<Endpoint>,
    /// SSH Config overrides, not needed
    pub ssh: Option<Ssh>,
    /// The level to log the daemon with
    pub loglevel: Option<String>,
}

impl EbbflowDaemonConfig {
    pub async fn check_permissions() -> Result<(), ConfigError> {
        let mut std = OpenOptions::new();
        std.write(true).create(true);
        let options = TokioOpenOptions::from(std);

        options.open(config_file_full()).await?;
        options.open(key_file_full()).await?;
        Ok(())
    }

    pub async fn load_from_file_or_new() -> Result<EbbflowDaemonConfig, ConfigError> {
        let cfg = match Self::load_from_file().await {
            Ok(existing) => existing,
            Err(e) => match e {
                ConfigError::Empty | ConfigError::FileNotFound => EbbflowDaemonConfig {
                    endpoints: vec![],
                    ssh: None,
                    loglevel: None,
                },
                _ => return Err(e),
            },
        };
        Ok(cfg)
    }

    pub async fn load_from_file() -> Result<EbbflowDaemonConfig, ConfigError> {
        Self::load_from_file_path(&config_file_full()).await
    }

    pub async fn load_from_file_path(p: &str) -> Result<EbbflowDaemonConfig, ConfigError> {
        let filebytes = fs::read(p).await?;

        let parsed: EbbflowDaemonConfig = match serde_yaml::from_slice(&filebytes[..]) {
            Ok(p) => match p {
                PossiblyEmptyEbbflowDaemonConfig::Empty => return Err(ConfigError::Empty),
                PossiblyEmptyEbbflowDaemonConfig::EbbflowDaemonConfig(c) => c,
            },
            Err(_e) => {
                info!("Error parsing configuration file");
                return Err(ConfigError::Parsing);
            }
        };

        Ok(parsed)
    }

    pub async fn save_to_file(&self) -> Result<(), ConfigError> {
        let b: String = match serde_yaml::to_string(self) {
            Ok(s) => s,
            Err(_e) => {
                info!("Error parsing current configuration into a YAML file");
                return Err(ConfigError::Parsing);
            }
        };

        Ok(fs::write(config_file_full(), b.as_bytes()).await?)
    }
}

/// An Endpoint to host. Provide the DNS name, and the local port.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Endpoint {
    /// The port your application runs on
    pub port: u16,
    /// The DNS name of the endpoint being hosted
    pub dns: String,
    /// the maximum amount of open connections, defaults to 200
    pub maxconns: u16,
    /// the maxmimum amount of idle connections to Ebbflow, will be capped at X
    pub maxidle: u16,
    /// Is this endpoint enabled or disabled?
    pub enabled: bool,
    /// Health Check, will automatically Enable and Disable based on pass/fail
    pub healthcheck: Option<HealthCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthCheck {
    /// The port to health check, defaults to the Endpoint's port.
    pub port: Option<u16>,
    /// How many successes before we consider this healthy? Defaults to 3.
    pub consider_healthy_threshold: Option<u16>,
    /// How many failures until we consider this unhealthy? Defaults to 3.
    pub consider_unhealthy_threshold: Option<u16>,
    /// The type of HealthCheck, only `TCP` is available now.
    pub r#type: HealthCheckType,
    /// How often (in seconds) the health check should be evaluated. Defaults to 5.
    pub frequency_secs: Option<u16>,
}

pub struct ConcreteHealthCheck {
    pub port: u16,
    pub consider_healthy_threshold: u16,
    pub consider_unhealthy_threshold: u16,
    pub r#type: HealthCheckType,
    pub frequency_secs: u16,
}

impl ConcreteHealthCheck {
    pub fn new(default_port: u16, hc: &HealthCheck) -> Self {
        Self {
            port: hc.port.unwrap_or(default_port),
            consider_healthy_threshold: hc.consider_healthy_threshold.unwrap_or(3),
            consider_unhealthy_threshold: hc.consider_unhealthy_threshold.unwrap_or(3),
            frequency_secs: hc.frequency_secs.unwrap_or(5),
            r#type: hc.r#type.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HealthCheckType {
    /// Just a simple TCP Connection, no data transfer only connect
    TCP,
}

impl FromStr for HealthCheckType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "tcp" => Ok(HealthCheckType::TCP),
            _ => Err(format!("Could not parse {} into a HealthCheckType", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ssh {
    /// the maximum amount of open connections
    pub maxconns: u16,
    /// The local port, defaults to 22
    pub port: u16,
    /// Is SSH enabled?
    pub enabled: bool,
    /// the maxmimum amount of idle connections to Ebbflow, will be capped at X
    pub maxidle: u16,
    /// The hostname to use as the target, defaults the OS provided Hostname
    pub hostname_override: Option<String>,
}

impl Ssh {
    pub fn new(enabled: bool, hostname: Option<String>) -> Ssh {
        Self {
            maxconns: 20,
            port: 22,
            enabled,
            hostname_override: hostname,
            maxidle: 5,
        }
    }
}
