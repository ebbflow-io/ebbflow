use serde::{Serialize, Deserialize};

#[derive(Debug)]
pub enum ConfigError {
    Parsing,
    FileNotFound,
    FilePermissions,
    AlreadyExists,
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
    /// Should SSH be used?
    pub enable_ssh: bool,
    /// SSH Config overrides, not needed
    pub ssh: Option<Ssh>,
}

impl EbbflowDaemonConfig {
    // pub fn add_and_save_endpoint_config(endpoint: Endpoint) -> Result<(), ConfigError> {
    //     // load config
    //     // add endpoint
    //         // Check if existing
    //     // save config
    //     todo!()
    // }
    // pub fn remove_and_save_endpoint_config(dns: &str) -> Result<bool, ConfigError> {
    //     // load config
    //     // remove endpoint
    //     // save config
    //     todo!()
    // }
    pub async fn load_from_file() -> Result<EbbflowDaemonConfig, ConfigError> {
        Ok(EbbflowDaemonConfig {
            key: "asdf".to_string(),
            endpoints: vec![
                Endpoint {
                    port: 8000,
                    dns: "ebbflow.io".to_string(),
                    maxconns: 1000,
                    idleconns_override: None,
                    address_override: None,
                }
            ],
            enable_ssh: false,
            ssh: None,
        })
        //Err(ConfigError::FileNotFound)
    }
    // pub async fn save_to_file(&self) -> Result<(), ConfigError> {
    //     Err(ConfigError::FileNotFound)
    // }
}

/// An Endpoint to host. Provide the DNS name, and the local port. Optionally override the local address,
/// which defaults to 127.0.0.1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Endpoint {
    /// The port your application runs on
    pub port: u16,
    /// The DNS name of the endpoint being hosted
    pub dns: String,
    /// the maximum amount of open connections, defaults to 1000
    pub maxconns: u16,
    /// the maxmimum amount of idle connections to Ebbflow, will be capped at 100
    pub idleconns_override: Option<usize>,
    /// The address the application runs on locally, defaults to 127.0.0.1
    pub address_override: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ssh {
    /// the maximum amount of open connections
    maxconns: u16,
    /// The local port, defaults to 22
    port: u16,
    /// The hostname to use as the target, defaults the OS provided Hostname
    hostname_override: Option<String>,
}