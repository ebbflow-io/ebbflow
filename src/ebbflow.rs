#[macro_use]
extern crate log;
extern crate env_logger;
use ebbflow::config::{ConfigError, EbbflowDaemonConfig, Endpoint, Ssh};
use clap::Clap;
use std::io;
use std::io::Error as IoError;

#[derive(Debug, Clap)]
struct Opts {
    #[clap(subcommand)]
    subcmd: SubCommand,
}

#[derive(Debug, Clap)]
enum SubCommand {
    /// Run the interactive initialization, retrieves a host key and sets up basic settings
    #[clap()]
    Init,
    /// Enable endpoint(s) or SSH proxying
    #[clap()]
    Enable(EnableDisableArgs),
    /// Disable endpoint(s) or SSH proxying
    #[clap()]
    Disable(EnableDisableArgs),
    // Retrieve the status of the Daemon. (NOTE: Output subject to change)
    // #[clap()]
    // Status,
    // Prints the current configuration
    // #[clap()]
    // PrintConfig,
}

#[derive(Debug, Clap)]
struct EndpointDns {
    dns: String,
}

#[derive(Debug, Clap)]
enum EnableDisableArgs {
    /// This action will apply to the SSH proxy
    Ssh,
    /// This action will apply to all endpoints
    AllEndpoints,
    /// This action will apply to just the specified endpoint
    Endpoint(EndpointDns),
}

#[derive(Debug, Clap)]
struct _InitArgs {
    /// Should SSH be enabled or disabled
    #[clap(default_value="true", parse(try_from_str))]
    enable_ssh: bool,

    /// Specify the account id for this host
    #[clap()]
    account_id: Option<String>,

    // endpoints: Vec<EndpointArg>,
}

#[derive(Debug, Clap)]
struct EndpointArg {
    port: u16,
    dns: String,
}

#[derive(Debug)]
enum CliError {
    ConfigError(ConfigError),
    StdIoError(IoError),
}

impl From<IoError> for CliError {
    fn from(v: IoError) -> Self {
        CliError::StdIoError(v)
    }
}

impl From<ConfigError> for CliError {
    fn from(v: ConfigError) -> Self {
        CliError::ConfigError(v)
    }
}

#[tokio::main]
async fn main() {
    let opts: Opts = Opts::parse();
    println!("Hey, not sleeping done now bye\n{:#?}", opts);

    let result: Result<(), CliError> = match opts.subcmd {
        SubCommand::Enable(args) => enabledisable(true, args).await.into(),
        SubCommand::Disable(args) => enabledisable(false, args).await.into(),
        SubCommand::Init => init().await,
    };

    match result {
        Ok(_) => {},
        Err(e) => match e {
            CliError::ConfigError(ce) => match ce {
                ConfigError::FilePermissions => exiterror("Permissions issue regarding configuration file, are you running this with elevated privelages?"),
                ConfigError::FileNotFound => exiterror("Expected configuration file not found, has initialization occurred yet?"),
                ConfigError::Parsing => exiterror("Failed to parse configuration properly, please notify Ebbflow"),
                ConfigError::Unknown(s) => exiterror(&format!("Unexpected error: {}, Please notify Ebbflow", s)),
            }
            _ => todo!()
        }
    }
}

fn exiterror(s: &str) {
    eprintln!("ERROR: {}", s);
    std::process::exit(1);
}

// Uses AccountId, creates a key, then prompts for endpoint/port combos and if SSH should be enabled
async fn init() -> Result<(), CliError> {
    EbbflowDaemonConfig::check_permissions().await?;

    let mut yn = String::new();
    println!("Would you like to have the SSH proxy enabled for this host? Please type 'yes', 'no', 'y', or 'n'");
    let enablessh = loop {
        io::stdin().read_line(&mut yn)?;
        match extract_yn(&yn) {
            Some(enabled) => break enabled,
            None => {}
        }
        println!("Could not parse {} into yes or no (or y or n), please retry", yn.trim());
        yn = String::new();
    };

    let mut account_id = String::new();
    loop {
        println!("Please enter your account id (viewable in the Ebbflow console)");
        io::stdin().read_line(&mut account_id)?;
        if account_id.len() > 10 || account_id.len() == 0 || account_id.is_empty() || !is_alphanumeric(&account_id) {
            println!("The account id you entered ({}) does not seem correct, please enter again. AccountIds are short and alphanumeric, e.g. a1b2c3", account_id.trim());
        } else {
            break
        }
        account_id = String::new();
    }
    let account_id = account_id.trim().to_string();

    let (url, keyid, secret_stuff) = create_key_request(&account_id).await?;
    print_url_instructions(url);

    let key = poll_key_creation(keyid, secret_stuff).await?;

    println!("Great! The key has been provisioned.");

    let cfg = EbbflowDaemonConfig {
        ssh: Ssh::new(enablessh),
        endpoints: vec![],
        key,
    };

    cfg.save_to_file().await?;

    Ok(())
}

fn extract_yn(yn: &str) -> Option<bool> {
    match yn.to_lowercase().trim() {
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

// Returns the String
async fn poll_key_creation(keyid: String, secret_stuff: String) -> Result<String, CliError> {
    Ok("ebb_hst_123412341".to_string())
}


fn print_url_instructions(url: String) {
    println!("Please go to {} to initialize the secret key for this host", url);
}

fn is_alphanumeric(s: &str) -> bool {
    for c in s.trim().chars() {
        if !c.is_ascii_alphanumeric() {
            return false
        }
    }
    true
}

// Gets a URL to display, and then the ID that will be used to poll with
async fn create_key_request(account_id: &str) -> Result<(String, String, String), CliError> {
    Ok(("https://ebbflow.io/init/a9di1nck13".to_string(), "a9di1nck13".to_string(), "secret_stuff".to_string()))
}

// init
// status (endpoints and ssh)
// get config file loc

async fn enabledisable(enable: bool, args: EnableDisableArgs) -> Result<(), CliError> {
    match args {
        EnableDisableArgs::Ssh => set_ssh_enabled(enable).await,
        EnableDisableArgs::AllEndpoints => set_endpoint_enabled(enable, None).await,
        EnableDisableArgs::Endpoint(e) => set_endpoint_enabled(enable, Some(&e.dns)).await,
    }
}

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

async fn set_endpoint_enabled(enabled: bool, dns: Option<&str>) -> Result<(), CliError> {
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

async fn set_ssh_enabled(enabled: bool) -> Result<(), CliError> {
    let mut existing = EbbflowDaemonConfig::load_from_file().await?;
    existing.ssh.enabled = enabled;
    existing.save_to_file().await?;
    Ok(())
}

async fn set_all_enabled(enabled: bool) -> Result<(), CliError> {
    set_endpoint_enabled(enabled, None).await?;
    set_ssh_enabled(enabled).await?;
    Ok(())
}
