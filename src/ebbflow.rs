#[macro_use]
extern crate log;

use clap::{crate_version, Clap};
use ebbflow::config::{
    getkey, setkey, ConfigError, EbbflowDaemonConfig, Endpoint, HealthCheck, HealthCheckType, Ssh,
};
use ebbflow::{
    certs::ROOTS,
    daemon::{
        connection::EndpointConnectionType, spawn_endpoint, EndpointArgs, HealthOverall, SharedInfo,
    },
    hostname_or_die,
    messagequeue::MessageQueue,
    DaemonEndpointStatus, DaemonRunner, DaemonStatus, DaemonStatusMeta,
};
use ebbflow_api::generatedmodels::{HostKeyInitContext, HostKeyInitFinalizationContext, KeyData};
use log::LevelFilter;
use regex::Regex;
use reqwest::StatusCode;
use std::io;
use std::time::Duration;
use std::{
    collections::VecDeque,
    fmt::{Display, Formatter},
    io::Error as IoError,
    sync::Arc,
};

const DEFAULT_SSH_CONNS: u16 = 20;
const DEFAULT_SSH_IDLE: u16 = 3;
const DEFAULT_EBBFLOW_API_ADDR: &str = "https://api.ebbflow.io/v1";
const DEFAULT_EBBFLOW_SITE_ADDR: &str = "https://ebbflow.io/init";

#[derive(Debug, Clap)]
#[clap(
    version = crate_version!(),
    max_term_width = 120,
    about = "The command line interface to control the Ebbflow proxy. The proxy runs in the background in the daemon in most cases, and most of the commands are used to modify this daemon.\n\n- General Info:\t\t\thttps://ebbflow.io/documentation#client\n- Troubleshooting Tips: \thttps://ebbflow.io/documentation#troubleshooting\n- Source Code:\t\t\thttps://github.com/ebbflow-io/ebbflow\n\nHINT: \n    `ebbflow status` can be very helpful!"
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
    /// Retrieve the status of the background daemon, helpful in debugging (NOTE: Output subject to change)
    Status,
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
    /// Modify the log level of the daemon
    LogLevel(LogLevelSubCommand),
}

#[derive(Debug, Clap)]
struct LogLevelSubCommand {
    #[clap(subcommand)]
    subcommand: LogLevelSubCommands,
}

#[derive(Debug, Clap)]
enum LogLevelSubCommands {
    /// Resets the log level to the default
    Reset,
    /// Sets the log level (restart needed)
    Set(LogLevelArgs),
}

#[derive(Debug, Clap)]
struct LogLevelArgs {
    /// Specify the level, e.g. Error, Warn, Info, Debug, Trace
    #[clap(short, long)]
    level: LevelFilter,
}

#[derive(Debug, Clap)]
struct InitArgs {
    /// Non interactive, you MUST provide the EBB_KEY environment variable!
    /// At this time, no other config options are taken, you can change/set settings
    /// with `ebbflow config SUBCOMMAND`.
    #[clap(short, long)]
    non_interactive: bool,
}

/// The arguments for the run-blocking command. Provide either --port, --dns, and other options, OR provide --file to point to a config file instead
#[derive(Debug, Clap)]
#[clap(setting = AppSettings::SubcommandsNegateReqs)]
#[clap(
    override_usage = "Either\tebbflow run-blocking --config_file <config_file>\n\tOr\tebbflow run-blocking --dns <dns> --port <port>"
)]
struct RunBlockingArgs {
    /// The location of a file with the ebbflow config, for help see https://ebbflow.io/documentation#config or the client on GitHub
    #[clap(name="config_file", env="EBB_CFG_FILE", long, required_unless_all(&["dns", "port"]), display_order=1, next_line_help=true)]
    config_file: Option<String>,
    /// The endpoint, e.g. example.com
    #[clap(
        name = "dns",
        short,
        long,
        required_unless("config_file"),
        requires("port"),
        display_order = 1,
        next_line_help = true
    )]
    dns: Option<String>,
    /// The local port, e.g. 80 or 7000
    #[clap(
        name = "port",
        short,
        long,
        required_unless("config_file"),
        requires("dns"),
        display_order = 1,
        next_line_help = true
    )]
    port: Option<u16>,
    /// The maximum amount of connections allowed
    #[clap(long, display_order = 1, next_line_help = true)]
    maxconns: Option<u16>,
    /// How many idle connections to keep open
    #[clap(long, display_order = 1, next_line_help = true)]
    maxidle: Option<u16>,
    /// Log level. Debug, Info, Warn, Error, (NOTE: Output subject to change)
    #[clap(long, display_order = 1, next_line_help = true)]
    loglevel: Option<LevelFilter>,
    /// This allows you to override the DNS value used to connect to Ebbflow, addr is required as well
    #[clap(long, hidden = true, next_line_help = true)]
    ebbflow_dns: Option<String>,
    /// This allows you to override the IP address of Ebbflow, dns is required as well
    #[clap(long, hidden = true, next_line_help = true)]
    ebbflow_addr: Option<String>,
    /// REQUIRED FOR HEALTH CHECKING: the port to check the health of
    #[clap(long, display_order = 2, next_line_help = true)]
    healthcheck_port: Option<u16>,
    /// How many successful health checks must pass to be considered healthy, defaults to 3
    #[clap(long, display_order = 2, next_line_help = true)]
    healthcheck_consider_healthy: Option<u16>,
    /// How many failed health checks must fail to be considered unhealthy, defaults to 3
    #[clap(long, display_order = 2, next_line_help = true)]
    healthcheck_consider_unhealthy: Option<u16>,
    /// How often the health check is ran in seconds, defaults to 5 seconds
    #[clap(long, display_order = 2, next_line_help = true)]
    healthcheck_frequency_secs: Option<u16>,
}

#[derive(Debug, Clap)]
struct EndpointDns {
    dns: String,
}

#[derive(Debug, Clap)]
struct SetupSshArgs {
    /// The port the SSH daemon runs on, default 22
    #[clap(long)]
    port: Option<u16>,
    /// The hostname to use, default is taken from OS
    #[clap(long)]
    hostname: Option<String>,
    /// The max number of connections, default 20
    #[clap(long)]
    maxconns: Option<u16>,
    /// the maxmimum amount of idle connections to Ebbflow, will be capped
    #[clap(long)]
    maxidle: Option<u16>,
    /// provide this if you'd like to initially have the SSH proxy disabled
    #[clap(short, long)]
    disabled: bool,
}

#[derive(Debug, Clap)]
struct AddEndpointArgs {
    /// The DNS value of this endpoint
    #[clap(short, long)]
    dns: String,
    /// The port the local service runs on
    #[clap(short, long)]
    port: u16,
    /// the maximum amount of open connections, defaults to 5000
    #[clap(long)]
    maxconns: Option<u16>,
    /// the maxmimum amount of idle connections to Ebbflow, will be capped
    #[clap(long)]
    maxidle: Option<usize>,
    /// provide this if you'd like to initially have this endpoint be disabled
    #[clap(long)]
    disabled: bool,
    /// REQUIRED FOR HEALTH CHECKING: the port to check the health of
    #[clap(long)]
    healthcheck_port: Option<u16>,
    /// How many successful health checks must pass to be considered healthy, defaults to 3
    #[clap(long)]
    healthcheck_consider_healthy: Option<u16>,
    /// How many failed health checks must fail to be considered unhealthy, defaults to 3
    #[clap(long)]
    healthcheck_consider_unhealthy: Option<u16>,
    /// How often the health check is ran in seconds, defaults to 5 seconds
    #[clap(long)]
    healthcheck_frequency_secs: Option<u16>,
}

#[derive(Debug, Clap)]
struct RemoveEndpointArgs {
    /// The DNS value of the endpoint to remove and shut down.
    dns: String,
    /// If you want this to succeed in case it didn't exist already, pass this
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
use tokio::time::delay_for;

#[tokio::main]
async fn main() {
    let app = Opts::into_app();
    let app = app.setting(AppSettings::VersionlessSubcommands);
    let app = app.setting(AppSettings::SubcommandsNegateReqs);
    let opts: Opts = Opts::from_arg_matches(&app.get_matches());
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
            Config::LogLevel(levelargs) => setloglevel(levelargs).await,
        },
        SubCommand::Init(args) => init(&addr, args).await,
        // SubCommand::RunBlocking(args) if args.file.is_some() => {
        //     todo!()
        // }
        SubCommand::RunBlocking(args) => run_blocking(args).await,
        SubCommand::Status => status().await,
    };

    match result {
        Ok(_) => {},
        Err(e) => match e {
            CliError::ConfigError(ce) => match ce {
                ConfigError::FilePermissions => exiterror("Permissions issue regarding configuration file, are you running this with elevated privelages?"),
                ConfigError::FileNotFound => exiterror("Expected configuration file not found, has initialization occurred yet?"),
                ConfigError::Parsing => exiterror("Failed to parse configuration properly, please notify Ebbflow"),
                ConfigError::Unknown(s) => exiterror(&format!("Unexpected error: {}, Please notify Ebbflow", s)),
                ConfigError::Empty => exiterror("The configuration file is empty"),
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
                Some(k) => k.trim().to_string(),
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

    println!("Would you like to have the SSH proxy enabled for this host? (y/yes n/no)");
    let enablessh = get_yn()?;

    let mut hostname: Option<String> = None;

    let sshcfg = if enablessh {
        println!("The hostname {} will be used as the hostname target for the SSH proxy, is that ok? (y/yes n/no)", defaulthostname);
        if !get_yn()? {
            let r = Regex::new(r"^[-\.[:alnum:]]{1,220}$").unwrap();
            loop {
                println!("Enter the hostname to be used as the target hostname for the SSH proxy");
                let newhn = get_string()?;
                if !r.is_match(newhn.as_str()) {
                    println!("The provided name does not appear to be a valid hostname. Must be alphanumeric and only have periods or dashes (-).");
                } else {
                    println!("The name {} will be used. Is that ok? (y/yes n/no)", newhn);

                    if get_yn()? {
                        hostname = Some(newhn);
                        break;
                    }
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
    setkey(&key).await?;
    println!("Great! The key has been provisioned.");

    let cfg = EbbflowDaemonConfig {
        ssh: sshcfg,
        endpoints: vec![],
        loglevel: None,
    };

    cfg.save_to_file().await?;
    println!(
        "\nInitialization complete. To set up and endpoint, use `ebbflow config add-endpoint`."
    );

    Ok(())
}

fn get_string() -> Result<String, CliError> {
    let mut yn = String::new();
    io::stdin().read_line(&mut yn)?;
    Ok(yn.trim().to_string())
}

fn get_yn() -> Result<bool, CliError> {
    Ok(loop {
        let yn = get_string()?;
        if let Some(enabled) = extract_yn(&yn) {
            break enabled;
        }
        println!(
            "Could not parse {} into (y/yes n/no), please retry",
            yn.trim()
        );
    })
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
    use std::io::Write;
    print!("Waiting for key creation to be completed..");
    let _ = std::io::stdout().flush();
    let client = reqwest::Client::new();
    loop {
        delay_for(Duration::from_secs(8)).await;
        match client
            .post(&format!("{}/hostkeyinit/{}", addr, finalizeme.id))
            .json(&finalizeme)
            .send()
            .await
        {
            Ok(response) => match response.status() {
                StatusCode::OK => {
                    println!();
                    let _ = std::io::stdout().flush();
                    let keydata: KeyData = response.json().await?;

                    // Hacky sleep, allows for policies/roles to be attached to this key before we attempt to authenticate
                    // or else we can warm the cache with policy-less/role-less data, and auth will fail, causing pain for the customer.
                    // Yes, this happened in testing!
                    delay_for(Duration::from_millis(2_500)).await;
                    return Ok(keydata.key);
                }
                StatusCode::ACCEPTED => {
                    print!(".");
                    let _ = std::io::stdout().flush();
                }
                StatusCode::NOT_FOUND => {}
                _ => println!(
                    "\tUnexpected status code from Ebbflow, will retry, weird! {}",
                    response.status()
                ),
            },
            Err(e) => println!(
                "\tError polling key creation, will just retry. For debugging: {:?}",
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

async fn enabledisable(enable: bool, args: &EnableDisableTarget) -> Result<(bool, bool), CliError> {
    match args {
        EnableDisableTarget::Ssh => set_ssh_enabled(enable).await,
        EnableDisableTarget::AllEndpoints => set_endpoint_enabled(enable, None).await,
        EnableDisableTarget::Endpoint(e) => set_endpoint_enabled(enable, Some(&e.dns)).await,
    }
}

async fn set_endpoint_enabled(enabled: bool, dns: Option<&str>) -> Result<(bool, bool), CliError> {
    let mut existing = EbbflowDaemonConfig::load_from_file_or_new().await?;

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

async fn setloglevel(levelargs: LogLevelSubCommand) -> Result<(), CliError> {
    let mut existing = EbbflowDaemonConfig::load_from_file_or_new().await?;
    existing.loglevel = match levelargs.subcommand {
        LogLevelSubCommands::Reset => None,
        LogLevelSubCommands::Set(l) => Some(l.level.to_string()),
    };
    existing.save_to_file().await?;
    println!("Log level set, now restart the daemon for this to take effect.");
    Ok(())
}

async fn set_ssh_enabled(enabled: bool) -> Result<(bool, bool), CliError> {
    let mut existing = EbbflowDaemonConfig::load_from_file_or_new().await?;
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
    let hc = match args.healthcheck_port {
        Some(p) => Some(HealthCheck {
            frequency_secs: args.healthcheck_frequency_secs,
            consider_healthy_threshold: args.healthcheck_consider_healthy,
            consider_unhealthy_threshold: args.healthcheck_consider_unhealthy,
            r#type: HealthCheckType::TCP,
            port: Some(p),
        }),
        None => None,
    };

    let newendpoint = Endpoint {
        port: args.port,
        dns: args.dns,
        maxconns: args.maxconns.unwrap_or(5000),
        maxidle: args.maxidle.unwrap_or(40) as u16,
        enabled: !args.disabled,
        healthcheck: hc,
    };

    let mut existing = EbbflowDaemonConfig::load_from_file_or_new().await?;

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
    let mut existing = EbbflowDaemonConfig::load_from_file_or_new().await?;

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
    let existing = EbbflowDaemonConfig::load_from_file_or_new().await?;
    println!("Endpoint Configuration");
    println!("----------------------");

    if !existing.endpoints.is_empty() {
        let mut max = 0;
        for e in existing.endpoints.iter() {
            max = std::cmp::max(max, e.dns.len());
        }

        println!(
            "{:width$}\tPort\tEnabled\tMaxConns\tMaxIdleConns\tHealthCheck",
            "DNS",
            width = max
        );
        for e in existing.endpoints {
            println!(
                "{:width$}\t{}\t{}\t{}\t\t{}\t\t{}",
                e.dns,
                e.port,
                e.enabled,
                e.maxconns,
                e.maxidle,
                match e.healthcheck {
                    None => "Not Configured".to_string(),
                    Some(hc) => {
                        format!(
                            "Port {} HealthyThreshold {} UnhealthyThreshold {} FreqSecs {}",
                            hc.port.unwrap_or(e.port),
                            hc.consider_healthy_threshold.unwrap_or(3),
                            hc.consider_unhealthy_threshold.unwrap_or(3),
                            hc.frequency_secs.unwrap_or(5)
                        )
                    }
                },
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
            .unwrap_or_else(hostname_or_die)
            .len();
        println!(
            "{:width$}\tPort\tEnabled\tMaxConns\tMaxIdleConns\t",
            "Hostname",
            width = max
        );
        println!(
            "{:width$}\t{}\t{}\t{}\t\t{}",
            sshcfg.hostname_override.unwrap_or_else(hostname_or_die),
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
    let mut existing = EbbflowDaemonConfig::load_from_file_or_new().await?;
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
        enabled: !args.disabled,
        hostname_override: args.hostname,
        maxidle: idle,
    });

    existing.save_to_file().await?;
    Ok(())
}

async fn remove_ssh() -> Result<(), CliError> {
    let mut existing = EbbflowDaemonConfig::load_from_file_or_new().await?;
    existing.ssh = None;
    existing.save_to_file().await?;
    Ok(())
}
async fn status() -> Result<(), CliError> {
    let addr = match ebbflow::config::read_addr().await {
        Ok(a) => a,
        Err(ConfigError::Empty) | Err(ConfigError::FileNotFound) => {
            return Err(CliError::Other(
                "Could not connect to daemon; was unable to determine daemon address".to_string(),
            ))
        }
        Err(e) => return Err(e.into()),
    };

    let status = match reqwest::get(&format!("http://{}/unstable_status", addr)).await {
        Ok(r) => match r.json::<ebbflow::DaemonStatus>().await {
            Ok(s) => s,
            Err(e) => return Err(CliError::Other(format!("Failure retrieving status from the local daemon! Very unexpected.\nError Message: {:?}", e)))
        },
        Err(e) => return Err(CliError::Other(format!("Failure retrieving status from the local daemon, it may not be running..\nError Message: {:?}", e))),
    };

    print_status(status);

    Ok(())
}

fn print_status(status: DaemonStatus) {
    println!(
        "Overall Health: {}",
        match status.meta {
            DaemonStatusMeta::Good => "Good",
            DaemonStatusMeta::Uninitialized => "Uninitialized",
        }
    );
    println!();
    println!("Endpoints");
    println!("----------------------");

    if !status.endpoints.is_empty() {
        let mut max = 0;
        for e in status.endpoints.iter() {
            max = std::cmp::max(max, e.0.len());
        }

        println!(
            "{:width$}\tEnabled\t\tCurrActiveConns\tCurrIdleConns\tHealth",
            "DNS",
            width = max
        );
        for (e, status) in status.endpoints {
            print_status_line(&e, status, max, true);
        }
    } else {
        println!("No endpoints known.");
    }
    println!();
    println!("SSH Proxy");
    println!("-----------------");

    if let Some((hostname, status)) = status.ssh {
        let max = hostname.len();
        println!(
            "{:width$}\tEnabled\t\tCurrActiveConns\tCurrIdleConns\t",
            "Hostname",
            width = max
        );
        print_status_line(&hostname, status, max, false);
    } else {
        println!("SSH configuration not known.");
    }

    println!();
    println!("Daemon Messages - Current Time {}", MessageQueue::now());
    println!("-----------------");
    if !status.messages.is_empty() {
        for (timestamp, message) in status.messages {
            println!("{} - {}", timestamp, message);
        }
    } else {
        println!("No daemon messages.");
    }
}

// Don't print health if SSH
fn print_status_line(
    endpoint_str: &str,
    status: DaemonEndpointStatus,
    max: usize,
    printhealth: bool,
) {
    let (enableddisabled, active, idle, health) = match status {
        DaemonEndpointStatus::Disabled => {
            ("Disabled", "".to_string(), "".to_string(), "".to_string())
        }
        DaemonEndpointStatus::Enabled {
            active,
            idle,
            health,
        } => (
            "Enabled",
            active.to_string(),
            idle.to_string(),
            if printhealth {
                match health {
                    Some(HealthOverall::NOT_CONFIGURED) => "Not Configured".to_string(),
                    Some(HealthOverall::HEALTHY(data)) => {
                        format!("Healthy - Old > New: {}", format_health_data_old_new(data))
                    }
                    Some(HealthOverall::UNHEALTHY(data)) => format!(
                        "Unhealthy - Old > New: {}",
                        format_health_data_old_new(data)
                    ),
                    None => "Not Configured".to_string(),
                }
            } else {
                "".to_string()
            },
        ),
    };
    println!(
        "{:width$}\t{}\t\t{}\t\t{}\t\t{}",
        endpoint_str,
        enableddisabled,
        active,
        idle,
        health,
        width = max
    );
}

fn format_health_data_old_new(data: VecDeque<(bool, u128)>) -> String {
    let mut s = "".to_string();
    for (check, _) in data.iter().rev() {
        s.push_str(if *check { "1" } else { "0" })
    }
    s
}

async fn run_blocking(args: RunBlockingArgs) -> Result<(), CliError> {
    let info = match (args.ebbflow_addr, args.ebbflow_dns) {
        (Some(addr), Some(dns)) => SharedInfo::new_with_ebbflow_overrides(
            addr.parse()
                .expect("Could not parse the ebbflow addr as ipv4"),
            dns.to_string(),
            ROOTS.clone(),
        )
        .await
        .map_err(|_| CliError::Other("Unable to create data necessary for ".to_string()))?,
        _ => SharedInfo::new()
            .await
            .map_err(|_| CliError::Other("Unable to create data necessary for ".to_string()))?,
    };

    env_logger::builder()
        .filter_level(args.loglevel.unwrap_or(LevelFilter::Info))
        .filter_module("rustls", log::LevelFilter::Error) // This baby gets noisy at lower levels
        .init();
    let key = match std::env::var("EBB_KEY").ok() {
        Some(k) => {
            debug!("EBB_KEY passed in and is being used");
            k.trim().to_string()
        }
        None => {
            info!("No EBB_KEY env var passed, trying to read from key file");
            getkey().await?
        }
    };

    if let Some(cfgfile) = args.config_file {
        let runner = Arc::new(DaemonRunner::new(Arc::new(info)));
        let cfg = match EbbflowDaemonConfig::load_from_file_path(cfgfile.trim()).await {
            Ok(cfg) => cfg,
            Err(e) => exiterror(&format!(
                "Error loading the config file you provided, path {} error: {:?}",
                cfgfile.trim(),
                e
            )),
        };
        runner.update_config(Some(cfg), Some(key)).await;
    } else {
        info.update_key(key);
        let hc = match args.healthcheck_port {
            Some(p) => Some(HealthCheck {
                frequency_secs: args.healthcheck_frequency_secs,
                consider_healthy_threshold: args.healthcheck_consider_healthy,
                consider_unhealthy_threshold: args.healthcheck_consider_unhealthy,
                r#type: HealthCheckType::TCP,
                port: Some(p),
            }),
            None => None,
        };

        spawn_endpoint(
            Arc::new(info),
            EndpointArgs {
                ctype: EndpointConnectionType::Tls,
                idleconns: args.maxidle.unwrap_or(10) as usize,
                maxconns: args.maxconns.unwrap_or(5_000) as usize,
                endpoint: args
                    .dns
                    .expect("logic error, please report bug to support@ebbflow.io"),
                port: args
                    .port
                    .expect("logic error, please report bug to support@ebbflow.io"),
                message_queue: Arc::new(MessageQueue::new()),
                healthcheck: hc,
            },
        )
        .await;
    }

    futures::future::pending::<()>().await;
    Ok(())
}
