#[macro_use]
extern crate log;
extern crate env_logger;

use clap::{value_t, App, Arg};
use futures::future::select;
use futures::future::Either;
use tokio::io::split;
use tokio::net::TcpStream;
// use futures::io::{AsyncReadExt, AsyncWriteExt};
use log::LevelFilter;
use rustls::DangerousWebPKIVerifierPinnedDNS;
use std::io::Error as IoError;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::prelude::*;
use tokio_io::split::{ReadHalf, WriteHalf};
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::webpki::DNSName;
use tokio_rustls::webpki::DNSNameRef;
use tokio_rustls::TlsConnector;

type Writer = Box<dyn AsyncWrite + Unpin + Send>;
type Reader = Box<dyn AsyncRead + Unpin + Send>;
type Clizzle = ClientTlsStream<TcpStream>; // Sick of typing it

trait ReaderWriterTrait: AsyncRead + AsyncWrite {}

const CONN_LOOP_SLEEP: u64 = 1000;

const PORT: &'static str = "PORT";
const ADDR: &'static str = "ADDR";
const KEY: &'static str = "KEY";
const CRT: &'static str = "CRT";
const DNS: &'static str = "DNS";
const NCONNS: &'static str = "NCONNS";
const DEFAULT_NCONNS: usize = 10;
const DEFAULT_ADDR: &'static str = "0.0.0.0";

#[tokio::main]
async fn main() {
    let matches = App::new("Ebbflow Client")
        .version("0.1")
        .about("Proxies ebbflow connections to your service")
        .arg(
            Arg::with_name(ADDR)
                .short("a")
                .long("local-addr")
                .value_name("ADDR")
                .help("The local ADDR to proxy requests to e.g. ADDR:PORT (defaults to 0.0.0.0)")
                .takes_value(true),
        )
        .arg(
            Arg::with_name(PORT)
                .short("p")
                .long("local-port")
                .value_name("PORT")
                .help("The local PORT to proxy requests to e.g. ADDR:PORT")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name(KEY)
                .short("k")
                .long("key")
                .value_name("KEY")
                .help("Path to file of client key for authenticating with Ebbflow")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name(CRT)
                .short("c")
                .long("cert")
                .value_name("CERT")
                .help("Path to file of client cert for authenticating with Ebbflow")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name(DNS)
                .short("d")
                .long("dns")
                .value_name("DNS")
                .help("The DNS entry of your endpoint, e.g. myawesomesite.com")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name(NCONNS)
                .long("numconns")
                .value_name("NUM")
                .help("The number of connections (default 10)")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("v")
                .short("v")
                .multiple(true)
                .help("Sets the level of verbosity"),
        )
        .get_matches();

    let key = matches.value_of(KEY).expect("must provide key");
    let crt = matches.value_of(CRT).expect("must provide cert");
    let addr = matches.value_of(ADDR).unwrap_or(DEFAULT_ADDR);
    let nconns = value_t!(matches, NCONNS, usize).unwrap_or(DEFAULT_NCONNS);
    let port = value_t!(matches, PORT, usize).unwrap_or_else(|e| e.exit());
    let local_addr = format!("{}:{}", addr, port)
        .parse()
        .expect("Unable to parse local address");
    let server_dns =
        DNSNameRef::try_from_ascii_str(matches.value_of(DNS).expect("must provide dns"))
            .expect("Unable to parse DNS name")
            .to_owned();

    let level = match matches.occurrences_of("v") {
        0 => LevelFilter::Warn,
        1 => LevelFilter::Info,
        2 => LevelFilter::Debug,
        3 | _ => LevelFilter::Trace,
    };

    env_logger::builder().filter_level(level).init();

    let server_addr = "34.210.53.235:7070"
        .parse()
        .expect("Unable to parse server address");

    let mut ccfg = ClientConfig::new();
    ccfg.root_store = ca();
    ccfg.set_single_client_cert(load_certs(crt), load_private_key(key));

    let verifier = DangerousWebPKIVerifierPinnedDNS::new(ebbflow_dns());
    let mut d = ccfg.dangerous();
    d.set_certificate_verifier(Arc::new(verifier));

    let server_client_config = Arc::new(ccfg);

    for _i in 0..nconns {
        let c = server_client_config.clone();
        tokio::spawn(loopy(server_addr, server_dns.clone(), c, local_addr));
    }

    //async_std::task::sleep(Duration::from_secs(60 * 60 * 60)).await;
    tokio::future::pending::<()>().await;
}

#[derive(Debug)]
enum ConnError {
    Io(IoError),
}

impl From<IoError> for ConnError {
    fn from(ioe: IoError) -> Self {
        ConnError::Io(ioe)
    }
}

async fn loopy(
    server_addr: SocketAddr,
    server_dns: DNSName,
    server_client_config: Arc<ClientConfig>,
    local_addr: SocketAddr,
) {
    loop {
        if let Err((msg, e)) = connection(
            server_addr,
            server_dns.as_ref(),
            server_client_config.clone(),
            local_addr,
        )
        .await
        {
            warn!("{}: {:?}", msg, e);
        }
        debug!(
            "Looping connection; Sleeping for {} millis before establishing a new connection",
            CONN_LOOP_SLEEP
        );
        async_std::task::sleep(Duration::from_millis(CONN_LOOP_SLEEP)).await;
    }
}

async fn connection(
    server_addr: SocketAddr,
    server_dns: DNSNameRef<'_>,
    server_client_config: Arc<ClientConfig>,
    local_addr: SocketAddr,
) -> Result<(), (&'static str, ConnError)> {
    // Get a connection to local
    debug!("Connecting to local addr");
    let (cr, cw) = connect_local(local_addr)
        .await
        .map_err(|e| ("Error establishing local connection", e))?;

    // Get a connection to Ebbflow
    debug!("Connecting to ebbflow");
    let (sr, sw) = connect_ebbflow(server_addr, server_dns, server_client_config)
        .await
        .map_err(|e| ("Error establishing connection with Ebbflow", e))?;

    // Serve
    debug!("Proxying connection");
    proxy(sr, sw, cr, cw).await.map_err(|e| ("sadf", e))
}

async fn connect_ebbflow(
    addr: SocketAddr,
    dns: DNSNameRef<'_>,
    cfg: Arc<ClientConfig>,
) -> Result<(Reader, Writer), ConnError> {
    let stream = TcpStream::connect(addr).await?;
    stream.set_keepalive(Some(Duration::from_secs(1)))?;
    stream.set_nodelay(true)?;

    let connector = TlsConnector::from(cfg);
    let tlsstream = connector.connect(dns, stream).await?;

    debug!("A connection has been established to ebbflow");

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
) -> Result<(), ConnError> {
    let s2c = sr.copy(&mut cw);
    let c2s = cr.copy(&mut sw);
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
MIIDEDCCAfgCCQDQ/SjS1+pBizANBgkqhkiG9w0BAQsFADBKMQswCQYDVQQGEwJV
UzELMAkGA1UECAwCV0ExEDAOBgNVBAcMB1NlYXR0bGUxDTALBgNVBAoMBHRlc3Qx
DTALBgNVBAsMBHRlc3QwHhcNMTkwODIwMTYzMDU5WhcNMjQwODE4MTYzMDU5WjBK
MQswCQYDVQQGEwJVUzELMAkGA1UECAwCV0ExEDAOBgNVBAcMB1NlYXR0bGUxDTAL
BgNVBAoMBHRlc3QxDTALBgNVBAsMBHRlc3QwggEiMA0GCSqGSIb3DQEBAQUAA4IB
DwAwggEKAoIBAQCm3WLJ0qwqjIbNrmC5xFDynparBRVLRbE4BrkWdAWJJ5hgBrxZ
0mg8U+eauEaWyBEx3f/kbzB3iWQiSa3Hkp1G9/C78EZ9OVLcbcpmWZ6ofY4ALxcV
rvluPBXFw6Dl6+mDQHR5svrIPuAsqFZuApn1mllkxqk7oycg0pDUeyZNDSDHfS59
510hCGpZ6Mc6nkUrGOLVGPraTqviQDSs/5PdIV30C2IIKIKqvHfOsCLcxCTXhv2m
UvvJYuFylw445B4UgkXbN1w6B9bEjLduL1ogSeVMr7LMMKGP5alnrC4ubKDnnUdP
lE3eBflK+6zpSqYZ3+Y6YweBWmMKT2QOtHS1AgMBAAEwDQYJKoZIhvcNAQELBQAD
ggEBAA1SYwAgeRia3D/FfvOOaybb8J4FYCg0Iz1kgp5QUAVEWDmQx0OU+gk/MpcI
SoO+bo51rNVhOAWBXV+x3aSIIr0Yu48wqJFX1iSCrdLEkR2KPDN4pNvYjMmyrBVr
h6QNRkBRDm0tM91DatGGymj0xCuO8udifK9mvAgFnmU3hOfCQOgSfhhltDCE2aG+
TvOO8QcYeTpsDjDZCtZrVCFhDJ4L7rvjgu0MjfY5ZTmo20EK3+53hDCWXRPytMsn
900+RQ70WSIHB7I633C9NFHVDWmRJh/TN7pvtp2lDELs6HbVNL/4/0+Q1Ost8VsZ
mwjQfTXKWLjWGhREmHMrEAIBI0k=
-----END CERTIFICATE-----
"
    .to_string();
    let mut crt = crt.as_bytes();
    let mut store = RootCertStore::empty();
    store
        .add_pem_file(&mut crt)
        .expect("should be able to parse CA crt");
    store
}

fn ebbflow_dns() -> DNSName {
    DNSNameRef::try_from_ascii_str("ebbflow.io")
        .expect("should be able to parse ebbflow domain")
        .to_owned()
}
