#[macro_use]
extern crate log;
extern crate env_logger;

use clap::{value_t, App, Arg};
use futures::future::select;
use futures::future::Either;
use log::LevelFilter;
use simplelog::*;
use std::convert::TryFrom;
use std::env;
use std::fs::File;
use std::io::Error as IoError;
use std::io::ErrorKind as IoErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::split;
use tokio::io::AsyncReadExt;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::prelude::*;
use tokio::time::timeout;
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::webpki::DNSName;
use tokio_rustls::webpki::DNSNameRef;
use tokio_rustls::TlsConnector;

type Writer = Box<dyn AsyncWrite + Unpin + Send>;
type Reader = Box<dyn AsyncRead + Unpin + Send>;
type Clizzle = ClientTlsStream<TcpStream>; // Sick of typing it
trait ReaderWriterTrait: AsyncRead + AsyncWrite {}

const CONN_LOOP_SLEEP: u64 = 4000;
const DEFAULT_EBB_WAIT: Duration = Duration::from_secs(60 * 60);
const PORT: &str = "port";
const ADDR: &str = "local-addr";
const KEY: &str = "key";
const DNS: &str = "dns";
const NCONNS: &str = "max-conns";
const DEFAULT_NCONNS: usize = 5;
const DEFAULT_ADDR: &str = "0.0.0.0";
const MAX_IDLE: usize = 25;

enum Command {
    TCP,
    SSH,
}

#[tokio::main]
async fn main() {
    let matches = App::new("Ebbflow Client")
        .version("0.4.1")
        .setting(clap::AppSettings::VersionlessSubcommands)
        .after_help("The Ebbflow client is intended to be simple to use in the interactive case,\nand powerful enough to support mutliple users and concurrent executions\nin other cases. See the online documentation for more guidance at\n\n- https://github.com/ebbflow-io/ebbflow\n- https://ebbflow.io.\n\nAny questions can be directed to support@ebbflow.io.\n\nThanks for using Ebbflow!")
        .about("\nProxies Ebbflow connections to your server.")
        .arg(
            Arg::with_name(KEY)
                .short("k")
                .long("key")
                .value_name("FILE")
                .global(true)
                .help("Path to file of client key for authenticating with Ebbflow")
                .takes_value(true)
        )
        .arg(
            Arg::with_name(NCONNS)
                .short("m")
                .long("max-conns")
                .value_name("NUM")
                .global(true)
                .help("The maximum number of connections (default 1)")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("logfile")
                .short("f")
                .long("logfile")
                .value_name("FILE")
                .global(true)
                .help("The file name to log to instead of STDOUT")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("localebb")
                .long("localebb")
                .global(true)
                .hidden(true),
        )
        .arg(
            Arg::with_name("envfile")
                .short("e")
                .long("envfile")
                .value_name("FILE")
                .global(true)
                .help("Path to file which sets environment variables to be used")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("v")
                .short("v")
                .global(true)
                .multiple(true)
                .help("Sets the level of verbosity (add v's for more logging, e.g. -vvv)"),
        )
        .subcommand(
            App::new("tcp")
                .about("use as TCP Proxy mode")
                .arg(Arg::with_name(DNS)
                            .short("d")
                            .long("dns")
                            .value_name("DNS")
                            .help("The DNS entry of your endpoint, e.g. myawesomesite.com")
                            .takes_value(true)
                )
                .arg(
                    Arg::with_name(ADDR)
                        .short("a")
                        .long("local-addr")
                        .value_name("ADDR")
                        .help("The local ADDR to proxy to (defaults to 0.0.0.0)")
                )
                .arg(
                    Arg::with_name(PORT)
                        .short("p")
                        .long("port")
                        .value_name("PORT")
                        .help("The local PORT to proxy requests to e.g. 8080")
                        .takes_value(true)
                )
        )
        .subcommand(
            App::new("ssh")
                .about("use as SSH proxy mode")
                .arg(
                    Arg::with_name("accountid")
                        .long("accountid")
                        .takes_value(true)
                        .help("The account id, needed if using SSH (e.g. 13d19a)"),
                )
                .arg(
                    Arg::with_name("hostname")
                        .long("hostname")
                        .takes_value(true)
                        .help("The hostname to be accessible by, defaults to server's hostname"),
                )
                .arg(
                    Arg::with_name("sshport")
                        .long("sshport")
                        .global(true)
                        .takes_value(true)
                        .hidden(true),
                )
        )
        .get_matches();

    if let Some(envfile) = get_var_string(&matches, "envfile", "EBB_ENVFILE", None, false) {
        if let Err(_e) = dotenv::from_filename(&envfile) {
            fail(&format!("The enviroment variable file provided, {}, was unable to be read", envfile));
        }
    }

    let command = if let None = matches.subcommand_name() {
        // No subcommand, so we should have the following env var set
        let mode = env::var("EBB_SINGLE_MODE")
            .map_err(|_| fail("Must provide a subcommand, or EBB_SINGLE_MODE env var. Also try --help"))
            .unwrap();
        match mode.to_lowercase().as_ref() {
            "ssh" => Command::SSH,
            "tcp" => Command::TCP,
            _ => fail(&format!("The provided EBB_SINGLE_MODE value is not supported: {}", mode)),
        }
    } else {
        match matches.subcommand() {
            ("tcp", Some(_tcp_matches)) => Command::TCP,
            ("ssh", Some(_ssh_matches)) => Command::SSH,
            _ => fail("Logic error in client, unreachable state (unrecognized subcommand)"),
        }
    };

    let (dns, localaddr, hostname) = match command {
        Command::TCP => {
            let tcp_matches = matches.subcommand_matches("tcp").map(|x| x.clone()).unwrap_or_else(|| clap::ArgMatches::new());
            let raw_dns: String = tcp_matches
                .value_of(DNS)
                .map(|x| x.to_string())
                .unwrap_or_else(|| {
                    env::var("EBB_DNS")
                        .map_err(|_| fail("Must provide dns, or EBB_DNS env var"))
                        .unwrap()
                });
            let server_dns = DNSNameRef::try_from_ascii_str(&raw_dns)
                .map_err(|_| fail("Unable to parse DNS name"))
                .unwrap()
                .to_owned();
            let addr = tcp_matches
                .value_of(ADDR)
                .map(|x| x.to_string())
                .unwrap_or_else(|| {
                    env::var("EBB_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string())
                });
            let port =
                value_t!(tcp_matches, PORT, usize).unwrap_or_else(|_| match env::var("EBB_PORT") {
                    Ok(s) => {
                        if s == "" {
                            fail("Must provide --port or EBB_PORT to ebbflow");
                        } else {
                            s.parse()
                                .map_err(|_| fail("cannot parse EBB_CONNS value to a number"))
                                .unwrap()
                        }
                    }
                    Err(_e) => fail("Must provide --port or EBB_PORT to ebbflow"),
                });

            let local_addr = format!("{}:{}", addr, port)
                .parse()
                .map_err(|_| fail("Unable to parse local address"))
                .unwrap();
            (server_dns, local_addr, None)
        }
        Command::SSH => {
            let ssh_matches = matches.subcommand_matches("ssh").map(|x| x.clone()).unwrap_or_else(|| clap::ArgMatches::new());
            let raw_hostname: String =
                get_var_string(&ssh_matches, "hostname", "EBB_HOST", None, false).unwrap_or_else(
                    || {
                        hostname::get()
                            .map_err(|_| fail("failed retrieving the hostname from the OS"))
                            .unwrap()
                            .into_string()
                            .map_err(|_| fail("failed to parse OS hostname"))
                            .unwrap()
                    },
                );
            let account_id: String =
                get_var_string(&ssh_matches, "accountid", "EBB_ACCOUNTID", None, true).unwrap();

            let sshport = value_t!(ssh_matches, "sshport", u16).unwrap_or(22);
            let endpoint = format!("{}.ebbflowssh", account_id);
            let dns = DNSNameRef::try_from_ascii_str(&endpoint)
                .map_err(|_| fail("Unable to parse DNS name"))
                .unwrap()
                .to_owned();
            (
                dns,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), sshport),
                Some(raw_hostname),
            )
        }
    };

    // GET GLOBAL ARGS
    let key: String = get_var_string(&matches, KEY, "EBB_KEY", None, true).unwrap();
    let max_conns =
        value_t!(matches, NCONNS, usize).unwrap_or_else(|_| match env::var("EBB_CONNS") {
            Ok(s) => {
                if s == "" {
                    DEFAULT_NCONNS
                } else {
                    s.parse().expect("cannot parse EBB_CONNS value to a number")
                }
            }
            Err(_e) => DEFAULT_NCONNS,
        });
    let level = match matches.occurrences_of("v") {
        0 => LevelFilter::Warn,
        1 => LevelFilter::Info,
        2 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };
    let use_local_ebbflow = matches.is_present("localebb");

    if let Some(filename) = get_var_string(&matches, "logfile", "EBB_LOGFILE", None, false) {
        CombinedLogger::init(vec![WriteLogger::new(
            level,
            Config::default(),
            File::create(&filename).unwrap(),
        )])
        .unwrap();
    } else {
        env_logger::builder().filter_level(level).init();
    }

    let mut ccfg = ClientConfig::new();
    ccfg.root_store = ca();
    ccfg.set_single_client_cert(load_certs(&key), load_private_key(&key))
        .unwrap();
    let server_client_config = Arc::new(ccfg);

    let numloopers = std::cmp::min(MAX_IDLE, max_conns);
    for _ in 0..numloopers {
        let d = dns.clone();
        let s = server_client_config.clone();
        let h = hostname.clone();
        tokio::spawn(async move {
            loopy(
                h,
                d,
                s,
                localaddr,
                server_addr(use_local_ebbflow),
                max_conns,
            )
            .await;
        });
    }
    debug!("Spawned {} loopers", numloopers);
    futures::future::pending().await
}

fn get_var_string(
    matches: &clap::ArgMatches,
    var_name: &str,
    env_var_name: &str,
    default: Option<String>,
    required: bool,
) -> Option<String> {
    match matches.value_of(var_name) {
        Some(s) => Some(s.to_string()),
        None => match std::env::var(env_var_name) {
            Ok(s) => Some(s),
            Err(_) => match default {
                Some(s) => Some(s),
                None => {
                    if required {
                        fail(&format!(
                            "Must provide --{} or {} environment variable",
                            var_name, env_var_name
                        ));
                    } else {
                        None
                    }
                }
            },
        },
    }
}

fn fail(s: &str) -> ! {
    eprintln!("ERROR: {}", s);
    std::process::exit(1);
}

#[derive(Debug)]
enum ConnError {
    Io(IoError),
    Other(String),
}

impl From<IoError> for ConnError {
    fn from(ioe: IoError) -> Self {
        ConnError::Io(ioe)
    }
}

fn server_addr(ebbflow_override: bool) -> SocketAddr {
    if ebbflow_override {
        return SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 7070);
    }
    if rand::random() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(75, 2, 123, 22)), 7070)
    } else {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(99, 83, 172, 111)), 7070)
    }
}

async fn loopy(
    hostname: Option<String>,
    server_dns: DNSName,
    server_client_config: Arc<ClientConfig>,
    local_addr: SocketAddr,
    ebbflow_addr: SocketAddr,
    max: usize,
) {
    let active_conns = Arc::new(AtomicUsize::new(0));
    loop {
        loop {
            let active = active_conns.load(Ordering::SeqCst);
            if active >= max {
                debug!("We have more active connections ({}) than our max ({}), we will not establish another connection with ebbflow until that is done", active, max);
                tokio::time::delay_for(Duration::from_millis(CONN_LOOP_SLEEP)).await;
            // TODO Jitter
            } else {
                debug!(
                    "We have less ({}) than our max ({}), we will continue",
                    active, max
                );
                break;
            }
        }

        match timeout(
            DEFAULT_EBB_WAIT,
            wait_for_conn(
                // TODO Jitter on timeout val
                ebbflow_addr,
                server_dns.as_ref(),
                server_client_config.clone(),
                hostname.as_deref(),
            ),
        )
        .await
        {
            Ok(result) => match result {
                Ok((r, w, i)) => {
                    let connscounter = active_conns.clone();
                    debug!("Got connection data from ebbflow, will connect locally");
                    tokio::spawn(async move {
                        connscounter.fetch_add(1, Ordering::SeqCst);
                        if let Err(e) = start_proxying_connection(r, w, i, local_addr).await {
                            debug!("Existing connection terminated: {:?}", e);
                        }
                        connscounter.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                Err((msg, e)) => {
                    debug!("Error waiting on an ebbflow connection");
                    warn!("{}: {:?}", msg, e);
                    tokio::time::delay_for(Duration::from_millis(CONN_LOOP_SLEEP)).await;
                    // TODO Jitter
                }
            },
            Err(_) => {
                debug!(
                    "The ebb connection timedout out after {:?}, will loop",
                    DEFAULT_EBB_WAIT
                );
            }
        }
    }
}

async fn wait_for_conn(
    server_addr: SocketAddr,
    server_dns: DNSNameRef<'_>,
    server_client_config: Arc<ClientConfig>,
    hostname: Option<&str>,
) -> Result<(Reader, Writer, Vec<u8>), (&'static str, ConnError)> {
    debug!("Connecting to ebbflow");
    let (mut sr, sw) = connect_ebbflow(server_addr, server_dns, server_client_config, hostname)
        .await
        .map_err(|e| ("Error establishing connection with Ebbflow", e))?;

    let mut initialbuf = [0; 256];
    let n = sr
        .read(&mut initialbuf[0..])
        .await
        .map_err(|e| ("Error with established connection to Ebbflow", e.into()))?;

    if n == 0 {
        return Err((
            "Read 0 bytes meaning a disconnect from ebbflow",
            ConnError::Io(IoError::from(IoErrorKind::ConnectionReset)),
        ));
    }
    Ok((sr, sw, initialbuf[0..n].to_vec()))
}

async fn start_proxying_connection(
    sr: Reader,
    sw: Writer,
    initially_read: Vec<u8>,
    local_addr: SocketAddr,
) -> Result<(), (&'static str, ConnError)> {
    // Get a connection to local
    debug!("Connecting to local addr");
    let (cr, cw) = connect_local(local_addr)
        .await
        .map_err(|e| ("Error establishing local connection", e))?;

    // Serve
    debug!("Proxying connection");
    proxy(sr, sw, cr, cw, &initially_read[..])
        .await
        .map_err(|e| ("sadf", e))
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(non_camel_case_types)]
pub enum Protocol {
    TCP_V0 = 0,
    SSH_V0 = 9,
}

impl TryFrom<u32> for Protocol {
    type Error = u32;
    fn try_from(n: u32) -> Result<Self, u32> {
        match n {
            _ if n == Protocol::SSH_V0 as u32 => Ok(Protocol::SSH_V0),
            _ if n == Protocol::TCP_V0 as u32 => Ok(Protocol::TCP_V0),
            _ => Err(n),
        }
    }
}

// 'Negotiates' to use the given protocol, we don't really negotiate we just tell them we want to use X and expect they accept X
async fn negotiate_protocol_only_ours(
    tlsstream: &mut ClientTlsStream<TcpStream>,
    protocol: Protocol,
) -> Result<(), ConnError> {
    let protobuf = (protocol as u32).to_be_bytes();
    tlsstream.write_all(&protobuf[..]).await?;
    let mut proto_to_use_buf = [0; 4];
    tlsstream.read_exact(&mut proto_to_use_buf[..]).await?;
    let proto_to_use_num = u32::from_be_bytes(proto_to_use_buf);
    match Protocol::try_from(proto_to_use_num) {
        Ok(n) if n == protocol => {
            debug!("Protocol used {:?}", n);
            Ok(())
        },
        Ok(_n) => Err(ConnError::Other(format!("They want to use protocol {} but as of now we don't support changing to that, from TCP_V0", proto_to_use_num))),
        Err(_e) => Err(ConnError::Other(format!("Unable to parse a protocol version from {}", proto_to_use_num))),
    }
}

async fn connect_ebbflow(
    addr: SocketAddr,
    dns: DNSNameRef<'_>,
    cfg: Arc<ClientConfig>,
    hostname: Option<&str>,
) -> Result<(Reader, Writer), ConnError> {
    let stream = TcpStream::connect(addr).await?;
    stream.set_keepalive(Some(Duration::from_secs(1)))?;
    stream.set_nodelay(true)?;

    let connector = TlsConnector::from(cfg);
    let mut tlsstream = connector.connect(dns, stream).await?;

    debug!("A connection has been established to ebbflow");

    // Protocol Negotiation
    match hostname {
        Some(h) => {
            negotiate_protocol_only_ours(&mut tlsstream, Protocol::SSH_V0).await?;
            let hostnamebuf = h.as_bytes();
            let lenbuf: [u8; 2] = (hostnamebuf.len() as u16).to_be_bytes();
            tlsstream.write_all(&lenbuf[..]).await?;
            tlsstream.write_all(&hostnamebuf[..]).await?;
            // SSH v0 involves stating the hostname and then that's it
        }
        None => {
            // TCP_V0
            negotiate_protocol_only_ours(&mut tlsstream, Protocol::TCP_V0).await?;
            // That's it!
        }
    }
    debug!("Protocol negotiaton complete");

    let (r, w) = split(tlsstream);

    Ok((dyner_r(r), dyner_w(w)))
}

fn dyner_r(readhalf: ReadHalf<Clizzle>) -> Reader {
    Box::new(readhalf)
}
fn dyner_w(writehalf: WriteHalf<Clizzle>) -> Writer {
    Box::new(writehalf)
}
fn dyner_rs(readhalf: ReadHalf<TcpStream>) -> Reader {
    Box::new(readhalf)
}
fn dyner_ws(writehalf: WriteHalf<TcpStream>) -> Writer {
    Box::new(writehalf)
}

async fn connect_local(addr: SocketAddr) -> Result<(Reader, Writer), ConnError> {
    let tcpstream = TcpStream::connect(addr).await?;
    tcpstream.set_keepalive(Some(Duration::from_secs(1)))?;
    tcpstream.set_nodelay(true)?;

    debug!("A connection has been established to the local server");

    let (r, w) = split(tcpstream);

    Ok((dyner_rs(r), dyner_ws(w)))
}

async fn proxy(
    mut sr: Reader,
    mut sw: Writer,
    mut cr: Reader,
    mut cw: Writer,
    initial: &[u8],
) -> Result<(), ConnError> {
    cw.write_all(initial).await?;
    let s2c = Box::pin(async move {
        tokio::io::copy(&mut sr, &mut cw).await?;
        Ok::<(), std::io::Error>(())
    });
    let c2s = Box::pin(async move {
        tokio::io::copy(&mut cr, &mut sw).await?;
        Ok::<(), std::io::Error>(())
    });
    debug!("A proxied connection has been established between the two parties");

    match select(s2c, c2s).await {
        Either::Left((_server_read_res, _c2s_future)) => debug!("Server reader finished first"),
        Either::Right((_client_read_res, _s2c_future)) => debug!("Client reader finished first"),
    }

    debug!("A proxied connection has terminated");

    Ok(())
}

use std::fs;
use std::io::BufReader;

pub fn load_certs(filename: &str) -> Vec<tokio_rustls::rustls::Certificate> {
    let certfile = fs::File::open(filename).expect("cannot open certificate file");
    let mut reader = BufReader::new(certfile);
    tokio_rustls::rustls::internal::pemfile::certs(&mut reader).unwrap()
}
pub fn load_private_key(filename: &str) -> tokio_rustls::rustls::PrivateKey {
    let rsa_keys = {
        let keyfile = fs::File::open(filename).expect("cannot open private key file");
        let mut reader = BufReader::new(keyfile);
        tokio_rustls::rustls::internal::pemfile::rsa_private_keys(&mut reader)
            .expect("file contains invalid rsa private key")
    };

    let pkcs8_keys = {
        let keyfile = fs::File::open(filename).expect("cannot open private key file");
        let mut reader = BufReader::new(keyfile);
        tokio_rustls::rustls::internal::pemfile::pkcs8_private_keys(&mut reader)
            .expect("file contains invalid pkcs8 private key (encrypted keys not supported)")
    };

    // prefer to load pkcs8 keys
    if !pkcs8_keys.is_empty() {
        pkcs8_keys[0].clone()
    } else {
        assert!(!rsa_keys.is_empty());
        rsa_keys[0].clone()
    }
}

fn ca() -> RootCertStore {
    let crt = "-----BEGIN CERTIFICATE-----
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
LkLFN9PcG2yzhHNkiW/U9sDY9At/N8nNdw==
-----END CERTIFICATE-----"
        .to_string();
    let mut crt = crt.as_bytes();
    let mut store = RootCertStore::empty();
    store
        .add_pem_file(&mut crt)
        .expect("should be able to parse CA crt");
    store
}
