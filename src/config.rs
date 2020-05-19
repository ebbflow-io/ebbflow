use crate::CONFIG_FILE;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::Error as IoError;
use tokio::io::ErrorKind;

#[derive(Debug)]
pub enum ConfigError {
    Parsing,
    FileNotFound,
    FilePermissions,
    AlreadyExists,
    Unknown(String),
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

/// Configuration for Ebbflow. Will be parsed to/from a YAML file located at
/// - /etc/ebbflow for Linux
/// - TBD for Windows
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbbflowDaemonConfig {
    /// The value of the host's key, e.g. ebb_hst_1324123412341234123
    pub key: String,
    /// A list of endpoints to host, see Endpoint
    pub endpoints: Vec<Endpoint>,
    /// SSH Config overrides, not needed
    pub ssh: Ssh,
}

impl EbbflowDaemonConfig {
    pub async fn load_from_file() -> Result<EbbflowDaemonConfig, ConfigError> {
        let filebytes = fs::read(CONFIG_FILE).await?;

        let parsed: EbbflowDaemonConfig = match serde_yaml::from_slice(&filebytes[..]) {
            Ok(p) => p,
            Err(_e) => {
                info!("Error parsing configuration file");
                return Err(ConfigError::Parsing);
            }
        };

        Ok(parsed)

        // Ok(EbbflowDaemonConfig {
        //     key: "ebb_hst_AicTDDfeUh0MnzZsKsn6hBiLlYP0vfutj5ztMd5KBh".to_string(),
        //     endpoints: vec![Endpoint {
        //         port: 41402,
        //         dns: "preview.ebbflow.io".to_string(),
        //         maxconns: 2,
        //         idleconns_override: Some(1),
        //         address_override: None,
        //         enabled: true,
        //     }],
        //     enable_ssh: true,
        //     ssh: None,
        // })
        //Err(ConfigError::FileNotFound)
    }
    pub async fn save_to_file(&self) -> Result<(), ConfigError> {
        let b: String = match serde_yaml::to_string(self) {
            Ok(s) => s,
            Err(_e) => {
                info!("Error parsing current configuration into a YAML file");
                return Err(ConfigError::Parsing);
            }
        };

        Ok(fs::write(CONFIG_FILE, b.as_bytes()).await?)
    }
}

/// An Endpoint to host. Provide the DNS name, and the local port. Optionally override the local address,
/// which defaults to 127.0.0.1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Endpoint {
    /// The port your application runs on
    pub port: u16,
    /// The DNS name of the endpoint being hosted
    pub dns: String,
    /// the maximum amount of open connections, defaults to 200
    pub maxconns: u16,
    /// the maxmimum amount of idle connections to Ebbflow, will be capped at X
    pub idleconns_override: Option<usize>,
    /// The address the application runs on locally, defaults to 127.0.0.1
    pub address_override: Option<String>,
    /// Is this endpoint enabled or disabled?
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ssh {
    /// the maximum amount of open connections
    pub maxconns: u16,
    /// The local port, defaults to 22
    pub port: u16,
    /// Is SSH enabled?
    pub enabled: bool,
    /// The hostname to use as the target, defaults the OS provided Hostname
    pub hostname_override: Option<String>,
}

impl Ssh {
    pub fn new(enabled: bool) -> Ssh {
        Self {
            maxconns: 20,
            port: 22,
            enabled,
            hostname_override: None,
        }
    }
}
