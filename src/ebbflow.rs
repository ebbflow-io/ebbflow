#[macro_use]
extern crate log;
extern crate env_logger;
use clap::Clap;
use ebb_api::generatedmodels::{HostKeyInitContext, HostKeyInitFinalizationContext, KeyData};
use ebbflow::config::{ConfigError, EbbflowDaemonConfig, Endpoint, Ssh};
use ebbflow::hostname_or_die;
use regex::Regex;
use reqwest::StatusCode;
use std::io;
use std::time::Duration;
use std::{
    fmt::{Display, Formatter},
    io::Error as IoError,
};

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

impl Display for EnableDisableArgs {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            EnableDisableArgs::Ssh => "Ssh".to_owned(),
            EnableDisableArgs::AllEndpoints => "All Endpoints".to_owned(),
            EnableDisableArgs::Endpoint(s) => s.dns.to_owned(),
        };
        write!(f, "{}", str)
    }
}

#[derive(Debug, Clap)]
struct _InitArgs {
    /// Should SSH be enabled or disabled
    #[clap(default_value = "true", parse(try_from_str))]
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
    Http(reqwest::Error),
}

impl From<reqwest::Error> for CliError {
    fn from(v: reqwest::Error) -> Self {
        CliError::Http(v)
    }
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

    let result: Result<(), CliError> = match opts.subcmd {
        SubCommand::Enable(args) => {
            handle_enable_disable_ret(&args, enabledisable(true, &args).await)
        }
        SubCommand::Disable(args) => {
            handle_enable_disable_ret(&args, enabledisable(false, &args).await)
        }
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
            CliError::StdIoError(e) => exiterror("Issue reading or writing to/from stdin or out, weird!"),
            CliError::Http(e) => exiterror(&format!("Problem making HTTP request to Ebbflow, please try again soon. For debugging: {:?}", e)),
        }
    }
}

fn handle_enable_disable_ret(
    args: &EnableDisableArgs,
    ret: Result<bool, CliError>,
) -> Result<(), CliError> {
    match ret {
        Ok(b) => {
            if b {
                Ok(())
            } else {
                if let EnableDisableArgs::AllEndpoints = args {
                    exiterror(&format!("Nothing to change, no endpoints are configured"));
                } else {
                    exiterror(&format!("The specified target does not exist: {}", args));
                }
            }
        }
        Err(e) => Err(e),
    }
}

fn exiterror(s: &str) -> ! {
    eprintln!("ERROR: {}", s);
    std::process::exit(1);
}

// Uses AccountId, creates a key, then prompts for endpoint/port combos and if SSH should be enabled
async fn init() -> Result<(), CliError> {
    EbbflowDaemonConfig::check_permissions().await?;
    let mut hostname = hostname_or_die();

    println!(
        "Would you like to have the SSH proxy enabled for this host? Please type yes, y, no, or n"
    );
    let enablessh = loop {
        let mut yn = String::new();
        io::stdin().read_line(&mut yn)?;
        match extract_yn(&yn) {
            Some(enabled) => break enabled,
            None => {}
        }
        println!(
            "Could not parse {} into yes or no (or y or n), please retry",
            yn.trim()
        );
    };

    if enablessh {
        println!("The hostname {} will be used to identify this host in the ebbflow proxy\ne.g. Clients will execute `ssh -J ebbflow.io {}`, is that ok?", hostname, hostname);
        if !loop {
            let mut yn = String::new();
            io::stdin().read_line(&mut yn)?;
            match extract_yn(&yn) {
                Some(yn) => break yn,
                None => {}
            }
            println!(
                "Could not parse {} into yes or no (or y or n), please retry",
                yn.trim()
            );
        } {
            println!("What would you like the host to be identified as for SSH proxy?");
            let r = Regex::new(r"^[-\.[:alnum:]]{1,220}$").unwrap();
            loop {
                let mut newhn = String::new();
                io::stdin().read_line(&mut newhn)?;
                let newhn = newhn.trim();
                if !r.is_match(&newhn) {
                    println!("The provided name does not appear to be a valid hostname. Must be alphanumeric and only have periods or dashes (-).");
                } else {
                    println!("The name {} will be used.", newhn);
                    hostname = newhn.to_owned();
                    break;
                }
            }
        }
    }

    let (url, finalizeme) = create_key_request(&hostname).await?;
    print_url_instructions(url);

    let key = poll_key_creation(finalizeme).await?;

    println!("Great! The key has been provisioned.");

    let mut ssh = Ssh::new(enablessh);
    ssh.hostname_override = Some(hostname);

    let cfg = EbbflowDaemonConfig {
        ssh,
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
async fn poll_key_creation(finalizeme: HostKeyInitFinalizationContext) -> Result<String, CliError> {
    print!("Waiting for key creation to be completed");
    let client = reqwest::Client::new();
    loop {
        tokio::time::delay_for(Duration::from_secs(5)).await;
        match client
            .post(&format!(
                "https://api.preview.ebbflow.io:4443/v1/hostkeyinit/{}",
                finalizeme.id
            ))
            .json(&finalizeme)
            .send()
            .await
        {
            Ok(response) => match response.status() {
                StatusCode::OK => {
                    use std::io::Write;
                    print!("\n");
                    let _ = std::io::stdout().flush();
                    let keydata: KeyData = response.json().await?;
                    return Ok(keydata.key);
                }
                StatusCode::ACCEPTED => {
                    use std::io::Write;
                    print!(".");
                    let _ = std::io::stdout().flush();
                }
                StatusCode::NOT_FOUND => {}
                _ => println!(
                    "Unexpected status code from Ebbflow, weird! {}",
                    response.status()
                ),
            },
            Err(e) => println!(
                "Error polling key creation, will just retry. For debugging: {:?}",
                e
            ),
        }
    }
}

fn print_url_instructions(url: String) {
    println!(
        "Please go to {} to initialize the secret key for this host\n",
        url
    );
}

// Gets a URL to display, and then the ID that will be used to poll with, and the secret to use when polling
async fn create_key_request(
    hostname: &str,
) -> Result<(String, HostKeyInitFinalizationContext), CliError> {
    let init = HostKeyInitContext::new(hostname.to_string());

    let client = reqwest::Client::new();
    let finalizeme: HostKeyInitFinalizationContext = client
        .post("https://api.preview.ebbflow.io:4443/v1/hostkeyinit")
        .json(&init)
        .send()
        .await?
        .json()
        .await?;

    Ok((
        format!("https://ebbflow.io/init/{}", finalizeme.id),
        finalizeme,
    ))
}

// init
// status (endpoints and ssh)
// get config file loc

async fn enabledisable(enable: bool, args: &EnableDisableArgs) -> Result<bool, CliError> {
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

async fn set_endpoint_enabled(enabled: bool, dns: Option<&str>) -> Result<bool, CliError> {
    let mut existing = EbbflowDaemonConfig::load_from_file().await?;

    let mut targeted_found = false;
    for e in existing.endpoints.iter_mut() {
        if let Some(actualdns) = dns {
            if actualdns == &e.dns {
                e.enabled = enabled;
                targeted_found = true;
            }
        } else {
            targeted_found = true;
            e.enabled = enabled;
        }
    }

    existing.save_to_file().await?;

    Ok(targeted_found)
}

async fn set_ssh_enabled(enabled: bool) -> Result<bool, CliError> {
    let mut existing = EbbflowDaemonConfig::load_from_file().await?;
    existing.ssh.enabled = enabled;
    existing.save_to_file().await?;
    Ok(true)
}
