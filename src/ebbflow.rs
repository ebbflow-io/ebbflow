#[macro_use]
extern crate log;

use clap::{crate_version, Clap};
use ebbflow::config::{getkey, setkey, ConfigError, EbbflowDaemonConfig, Endpoint, Ssh};
use ebbflow::{
    daemon::{connection::EndpointConnectionType, spawn_endpoint, EndpointArgs, SharedInfo},
    hostname_or_die,
};
use ebbflow_api::generatedmodels::{HostKeyInitContext, HostKeyInitFinalizationContext, KeyData};
use log::LevelFilter;
use regex::Regex;
use reqwest::StatusCode;
use std::io;
use std::time::Duration;
use std::{
    fmt::{Display, Formatter},
    io::Error as IoError,
    net::SocketAddrV4,
    sync::Arc,
};

const DEFAULT_SSH_CONNS: u16 = 10;
const DEFAULT_SSH_IDLE: u16 = 3;
const DEFAULT_EBBFLOW_API_ADDR: &str = "https://api.ebbflow.io/v1";
const DEFAULT_EBBFLOW_SITE_ADDR: &str = "https://ebbflow.io/init";

#[derive(Debug, Clap)]
#[clap(
    version = crate_version!(),
    max_term_width = 120,
    about = "The command line interface to control the Ebbflow proxy. The proxy runs in the background in the daemon in most cases, and most of the commands are used to modify this daemon.\n\nPlease see https://ebbflow.io/documentation#client for any questions."
)]
struct Opts {
    #[clap(subcommand)]
    subcmd: SubCommand,
    // #[clap(hidden = true)]
    // ebbflow_addr: Option<String>,
}

#[derive(Debug, Clap)]
enum SubCommand {
    /// Run the initialization, retrieves/sets the host key and sets up basic settings
    Init(InitArgs),
    /// Configure the background daemon
    Config(ConfigSubCommand),
    /// Run the ebbflow proxy now, in a blocking fashion, without the daemon.
    /// Typically used in container environments.
    /// Use `EBB_KEY` environment variable to pass the key in, or it will fallback to key from init
    RunBlocking(RunBlockingArgs),
}

#[derive(Debug, Clap)]
struct ConfigSubCommand {
    #[clap(subcommand)]
    config: Config,
}

#[derive(Debug, Clap)]
enum Config {
    /// Enable endpoint(s) or SSH proxying
    Enable(EnableDisableArgs),
    /// Disable endpoint(s) or SSH proxying
    Disable(EnableDisableArgs),
    /// Add a new endpoint
    AddEndpoint(AddEndpointArgs),
    /// Remove (and shut down) an endpoint
    RemoveEndpoint(RemoveEndpointArgs),
    /// Set up the SSH configuration. Defaults are provided for all arguments.
    SetupSsh(SetupSshArgs),
    /// Remove the SSH configuration, disabling the proxy
    RemoveSshConfiguration,
    /// Prints the current configuration (NOTE: Output subject to change)
    Print,
}

#[derive(Debug, Clap)]
struct InitArgs {
    /// Non interactive, you MUST provide the EBB_KEY environment variable!
    /// At this time, no other config options are taken, you can change/set settings
    /// with `ebbflow config SUBCOMMAND`.
    #[clap(short, long)]
    non_interactive: bool,
}

#[derive(Debug, Clap)]
struct RunBlockingArgs {
    /// The local port, e.g. 80 or 7000
    port: u16,
    /// The endpoint, e.g. example.com
    dns: String,
    /// The maximum amount of connections allowed
    #[clap(long)]
    maxconns: Option<u16>,
    /// How many idle connections to keep open
    #[clap(long)]
    maxidle: Option<u16>,
    /// Log level. Debug, Info, Warn (Default), NOTE: Output is subject to change!!
    #[clap(long)]
    loglevel: Option<LevelFilter>,
}

#[derive(Debug, Clap)]
struct EndpointDns {
    dns: String,
}

#[derive(Debug, Clap)]
struct SetupSshArgs {
    /// The max number of connections, default 10
    #[clap(long)]
    maxconns: Option<u16>,
    /// The port the SSH daemon runs on, default 22
    #[clap(long)]
    port: Option<u16>,
    /// The hostname to use, default is taken from OS
    #[clap(long)]
    hostname: Option<String>,
    /// the maxmimum amount of idle connections to Ebbflow, will be capped (NUM)
    #[clap(long)]
    maxidle: Option<u16>,
}

#[derive(Debug, Clap)]
struct AddEndpointArgs {
    /// The DNS value of this endpoint
    dns: String,
    /// The port the local service runs on (NUM)
    local_port: u16,
    /// the maximum amount of open connections, defaults to 200 (NUM)\
    #[clap(long)]
    maxconns: Option<u16>,
    /// the maxmimum amount of idle connections to Ebbflow, will be capped (NUM)
    #[clap(long)]
    maxidle: Option<usize>,
    // The address the application runs on locally, defaults to 127.0.0.1
    // address_override: Option<String>,
}

#[derive(Debug, Clap)]
struct RemoveEndpointArgs {
    /// The DNS value of the endpoint to remove and shut down.
    dns: String,
    /// If you want this to succeed in case it didn't exist already,pass this
    #[clap(short, long)]
    idempotent: bool,
}

#[derive(Debug, Clap)]
struct EnableDisableArgs {
    /// The target to enable or disable
    #[clap(subcommand)]
    target: EnableDisableTarget,
    /// If you want this to succeed in case it didn't exist already, pass this
    #[clap(short, long)]
    idempotent: bool,
}

#[derive(Debug, Clap)]
enum EnableDisableTarget {
    /// This action will apply to the SSH proxy
    Ssh,
    /// This action will apply to all endpoints
    AllEndpoints,
    /// This action will apply to just the specified endpoint
    Endpoint(EndpointDns),
}

impl Display for EnableDisableTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            EnableDisableTarget::Ssh => "Ssh".to_owned(),
            EnableDisableTarget::AllEndpoints => "All Endpoints".to_owned(),
            EnableDisableTarget::Endpoint(s) => s.dns.to_owned(),
        };
        write!(f, "{}", str)
    }
}

#[derive(Debug)]
enum CliError {
    ConfigError(ConfigError),
    StdIoError(IoError),
    Http(reqwest::Error),
    Other(String),
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
use clap::derive::FromArgMatches;
use clap::{derive::IntoApp, AppSettings};

#[tokio::main]
async fn main() {
    let app = Opts::into_app();
    let app = app.setting(AppSettings::VersionlessSubcommands);
    let opts: Opts = Opts::from_arg_matches(&app.get_matches());
    // let addr = opts
    //     .ebbflow_addr
    //     .unwrap_or_else(|| DEFAULT_EBBFLOW_API_ADDR.to_string());
    let addr = DEFAULT_EBBFLOW_API_ADDR.to_owned();

    let result: Result<(), CliError> = match opts.subcmd {
        SubCommand::Config(cfg) => match cfg.config {
            Config::Enable(args) => {
                handle_enable_disable_ret(true, &args, enabledisable(true, &args.target).await)
            }
            Config::Disable(args) => {
                handle_enable_disable_ret(false, &args, enabledisable(false, &args.target).await)
            }
            Config::AddEndpoint(args) => add_endpoint(args).await,
            Config::RemoveEndpoint(args) => remove_endpoint(args).await,
            Config::Print => printconfignokey().await,
            Config::SetupSsh(args) => setup_ssh(args).await,
            Config::RemoveSshConfiguration => remove_ssh().await,
        },
        SubCommand::Init(args) => init(&addr, args).await,
        SubCommand::RunBlocking(args) => run_blocking(args).await,
    };

    match result {
        Ok(_) => {},
        Err(e) => match e {
            CliError::ConfigError(ce) => match ce {
                ConfigError::FilePermissions => exiterror("Permissions issue regarding configuration file, are you running this with elevated privelages?"),
                ConfigError::FileNotFound => exiterror("Expected configuration file not found, has initialization occurred yet?"),
                ConfigError::Parsing => exiterror("Failed to parse configuration properly, please notify Ebbflow"),
                ConfigError::Unknown(s) => exiterror(&format!("Unexpected error: {}, Please notify Ebbflow", s)),
                ConfigError::Empty => exiterror("The configuration file is empty, please run initialization (ebbflow init) first"),
            }
            CliError::StdIoError(_e) => exiterror("Issue reading or writing to/from stdin or out, weird!"),
            CliError::Http(e) => exiterror(&format!("Problem making HTTP request to Ebbflow, please try again soon. For debugging: {:?}", e)),
            CliError::Other(e) => exiterror(&format!("Unexpected error: {}", e)),
        }
    }
}

fn handle_enable_disable_ret(
    enable: bool,
    args: &EnableDisableArgs,
    ret: Result<(bool, bool), CliError>,
) -> Result<(), CliError> {
    let word = if enable { "enabled" } else { "disabled" };

    match ret {
        Ok((found, mutated)) => match (found, mutated) {
            (true, true) => Ok(()),
            (true, false) => {
                if !args.idempotent {
                    exiterror(&format!(
                        "The specified target was not {}: {}",
                        word, args.target
                    ))
                } else {
                    Ok(())
                }
            }
            (false, _) => match (enable, args.idempotent) {
                (true, _) => exiterror(&format!(
                    "The specified target was not enabled as it is not configured: {}",
                    args.target
                )),
                (false, false) => exiterror(&format!(
                    "Unable to disable target {} as it is not configured",
                    args.target
                )),
                (false, true) => Ok(()),
            },
        },
        Err(e) => Err(e),
    }
}

fn exiterror(s: &str) -> ! {
    eprintln!("ERROR: {}", s);
    std::process::exit(1);
}

// Uses AccountId, creates a key, then prompts for endpoint/port combos and if SSH should be enabled
async fn init(addr: &str, args: InitArgs) -> Result<(), CliError> {
    if args.non_interactive {
        let key =
            match std::env::var("EBB_KEY").ok() {
                Some(k) => k,
                None => return Err(CliError::Other(
                    "Environment variable EBB_KEY not provided, required for non-interactive init"
                        .to_owned(),
                )),
            };

        setkey(&key).await?;

        Ok(())
    } else {
        init_interactive(addr).await
    }
}

async fn init_interactive(addr: &str) -> Result<(), CliError> {
    EbbflowDaemonConfig::check_permissions().await?;
    let defaulthostname = hostname_or_die();

    println!(
        "Would you like to have the SSH proxy enabled for this host? Please type yes, y, no, or n"
    );
    let enablessh = loop {
        let mut yn = String::new();
        io::stdin().read_line(&mut yn)?;
        if let Some(enabled) = extract_yn(&yn) {
            break enabled;
        }
        println!(
            "Could not parse {} into yes or no (or y or n), please retry",
            yn.trim()
        );
    };

    let mut hostname: Option<String> = None;

    let sshcfg = if enablessh {
        println!("The hostname {} will be used to identify this host in the ebbflow proxy\ne.g. Clients will execute `ssh -J ebbflow.io {}`, is that ok?", defaulthostname, defaulthostname);
        if !loop {
            let mut yn = String::new();
            io::stdin().read_line(&mut yn)?;
            if let Some(yn) = extract_yn(&yn) {
                break yn;
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
                    hostname = Some(newhn.to_owned());
                    break;
                }
            }
        }
        Some(Ssh::new(enablessh, hostname.clone()))
    } else {
        None
    };

    let usedname = hostname.unwrap_or_else(|| defaulthostname);

    let (url, finalizeme) = create_key_request(&usedname, addr).await?;
    print_url_instructions(url);

    let key = poll_key_creation(finalizeme, addr).await?;

    println!("Great! The key has been provisioned.");

    setkey(&key).await?;

    let cfg = EbbflowDaemonConfig {
        ssh: sshcfg,
        endpoints: vec![],
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
async fn poll_key_creation(
    finalizeme: HostKeyInitFinalizationContext,
    addr: &str,
) -> Result<String, CliError> {
    print!("Waiting for key creation to be completed");
    let client = reqwest::Client::new();
    loop {
        tokio::time::delay_for(Duration::from_secs(5)).await;
        match client
            .post(&format!("{}/hostkeyinit/{}", addr, finalizeme.id))
            .json(&finalizeme)
            .send()
            .await
        {
            Ok(response) => match response.status() {
                StatusCode::OK => {
                    use std::io::Write;
                    println!();
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
    addr: &str,
) -> Result<(String, HostKeyInitFinalizationContext), CliError> {
    let init = HostKeyInitContext::new(hostname.to_string());

    let client = reqwest::Client::new();
    let finalizeme: HostKeyInitFinalizationContext = client
        .post(&format!("{}/hostkeyinit", addr))
        .json(&init)
        .send()
        .await?
        .json()
        .await?;

    Ok((
        format!("{}/{}", DEFAULT_EBBFLOW_SITE_ADDR, finalizeme.id),
        finalizeme,
    ))
}

// init
// status (endpoints and ssh)
// get config file loc

async fn enabledisable(enable: bool, args: &EnableDisableTarget) -> Result<(bool, bool), CliError> {
    match args {
        EnableDisableTarget::Ssh => set_ssh_enabled(enable).await,
        EnableDisableTarget::AllEndpoints => set_endpoint_enabled(enable, None).await,
        EnableDisableTarget::Endpoint(e) => set_endpoint_enabled(enable, Some(&e.dns)).await,
    }
}

async fn set_endpoint_enabled(enabled: bool, dns: Option<&str>) -> Result<(bool, bool), CliError> {
    let mut existing = EbbflowDaemonConfig::load_from_file().await?;

    let mut targeted_found = false;
    let mut mutated = false;
    for e in existing.endpoints.iter_mut() {
        if let Some(actualdns) = dns {
            if actualdns == e.dns {
                if e.enabled == enabled {
                    mutated = false;
                } else {
                    e.enabled = enabled;
                    mutated = true;
                }
                targeted_found = true;
            }
        } else {
            mutated = true;
            targeted_found = true;
            e.enabled = enabled;
        }
    }

    existing.save_to_file().await?;

    Ok((targeted_found, mutated))
}

async fn set_ssh_enabled(enabled: bool) -> Result<(bool, bool), CliError> {
    let mut existing = EbbflowDaemonConfig::load_from_file().await?;
    let ret = if let Some(ref mut ssh) = &mut existing.ssh {
        if ssh.enabled == enabled {
            (true, false)
        } else {
            ssh.enabled = enabled;
            (true, true)
        }
    } else {
        (false, false)
    };
    existing.save_to_file().await?;
    Ok(ret)
}

async fn add_endpoint(args: AddEndpointArgs) -> Result<(), CliError> {
    let newendpoint = Endpoint {
        port: args.local_port,
        dns: args.dns,
        maxconns: args.maxconns.unwrap_or(500),
        maxidle: args.maxidle.unwrap_or(20) as u16,
        // address: args.address_override.unwrap_or_else(|| "127.0.0.1".to_string()),
        enabled: true,
    };

    let mut existing = EbbflowDaemonConfig::load_from_file().await?;

    // make sure it doesn't exist
    for e in existing.endpoints.iter() {
        if e.dns == newendpoint.dns {
            exiterror(&format!("An endpoint of name {} already exists. Please remove it first then create it again", e.dns));
        }
    }
    // Doesn't exist, add it
    existing.endpoints.push(newendpoint);
    existing.save_to_file().await?;
    Ok(())
}

async fn remove_endpoint(args: RemoveEndpointArgs) -> Result<(), CliError> {
    let mut existing = EbbflowDaemonConfig::load_from_file().await?;

    let mut deleted = false;
    // make sure it doesn't exist
    existing.endpoints.retain(|e| {
        if e.dns == args.dns {
            deleted = true;
            false
        } else {
            true
        }
    });

    // Not my finest but it'll do
    let ret = if deleted || args.idempotent {
        Ok(())
    } else {
        exiterror(&format!(
            "Endpoint {} does not exist and was not deleted",
            args.dns
        ))
    };

    existing.save_to_file().await?;
    ret
}

async fn printconfignokey() -> Result<(), CliError> {
    let existing = EbbflowDaemonConfig::load_from_file().await?;
    println!("Endpoint Configuration");
    println!("----------------------");

    if !existing.endpoints.is_empty() {
        let mut max = 0;
        for e in existing.endpoints.iter() {
            max = std::cmp::max(max, e.dns.len());
        }

        println!(
            "{:width$}\tPort\tEnabled\tMaxConns\tMaxIdleConns\t",
            "DNS",
            width = max
        );
        for e in existing.endpoints {
            println!(
                "{:width$}\t{}\t{}\t{}\t\t{}",
                e.dns,
                e.port,
                e.enabled,
                e.maxconns,
                e.maxidle,
                width = max
            );
        }
    } else {
        println!("No endpoints configured");
    }
    println!();
    println!("SSH Configuration");
    println!("-----------------");

    if let Some(sshcfg) = existing.ssh {
        let max = sshcfg
            .hostname_override
            .clone()
            .unwrap_or_else(|| hostname_or_die())
            .len();
        println!(
            "{:width$}\tPort\tEnabled\tMaxConns\tMaxIdleConns\t",
            "Hostname",
            width = max
        );
        println!(
            "{:width$}\t{}\t{}\t{}\t\t{}",
            sshcfg
                .hostname_override
                .unwrap_or_else(|| hostname_or_die()),
            sshcfg.port,
            sshcfg.enabled,
            sshcfg.maxconns,
            sshcfg.maxidle,
            width = max
        );
    } else {
        println!("SSH not configured");
    }

    Ok(())
}

async fn setup_ssh(args: SetupSshArgs) -> Result<(), CliError> {
    let mut existing = EbbflowDaemonConfig::load_from_file().await?;
    if let Some(_existing) = existing.ssh {
        exiterror("SSH configuration already set up, please remove it and then recreate it");
    }
    let idle = std::cmp::min(
        args.maxidle.unwrap_or_else(|| DEFAULT_SSH_IDLE),
        ebbflow::MAX_MAX_IDLE as u16,
    );

    existing.ssh = Some(Ssh {
        maxconns: args.maxconns.unwrap_or(DEFAULT_SSH_CONNS),
        port: args.port.unwrap_or(22),
        enabled: true,
        hostname_override: args.hostname,
        maxidle: idle,
    });

    existing.save_to_file().await?;
    Ok(())
}

async fn remove_ssh() -> Result<(), CliError> {
    let mut existing = EbbflowDaemonConfig::load_from_file().await?;
    existing.ssh = None;
    existing.save_to_file().await?;
    Ok(())
}

async fn run_blocking(args: RunBlockingArgs) -> Result<(), CliError> {
    let info = SharedInfo::new()
        .await
        .map_err(|_| CliError::Other(format!("Unable to create data necessary for ")))?;
    env_logger::builder()
        .filter_level(args.loglevel.unwrap_or(LevelFilter::Warn))
        .filter_module("rustls", log::LevelFilter::Error) // This baby gets noisy at lower levels
        .init();
    let key = match std::env::var("EBB_KEY").ok() {
        Some(k) => {
            debug!("Key passed in");
            k
        }
        None => {
            debug!("No key passed, trying cfg");
            getkey().await?
        }
    };

    info.update_key(key);

    let address = "127.0.0.1";
    let ip = address.parse().unwrap();
    let addr = SocketAddrV4::new(ip, args.port);

    spawn_endpoint(
        Arc::new(info),
        EndpointArgs {
            ctype: EndpointConnectionType::Tls,
            idleconns: args.maxidle.unwrap_or(10) as usize,
            maxconns: args.maxidle.unwrap_or(10_000) as usize,
            endpoint: args.dns,
            local_addr: addr,
        },
    )
    .await;

    futures::future::pending::<()>().await;
    Ok(())
}
