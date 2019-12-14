#[macro_use]
extern crate log;
extern crate env_logger;

use clap::{value_t, App, Arg};
use futures::future::select;
use futures::future::Either;
use log::LevelFilter;
use std::io::Error as IoError;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::split;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::prelude::*;
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
const DEFAULT_NCONNS: usize = 1;
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
                .help("The number of connections (default 1 (!!1!!!1!one!!))")
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

    let mut ccfg = ClientConfig::new();
    ccfg.root_store = ca();
    ccfg.set_single_client_cert(load_certs(crt), load_private_key(key));
    let server_client_config = Arc::new(ccfg);

    for _i in 0..nconns {
        let c = server_client_config.clone();
        tokio::spawn(loopy(server_dns.clone(), c, local_addr));
    }
    //async_std::task::sleep(Duration::from_secs(60 * 60 * 60)).await;
    futures::future::pending::<()>().await;
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

fn server_addr() -> SocketAddr {
    if rand::random() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(75, 2, 123, 22)), 7070)
    //SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0,0,0,0)), 7070)
    } else {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(99, 83, 172, 111)), 7070)
        //SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0,0,0,0)), 7070)
    }
}

async fn loopy(
    server_dns: DNSName,
    server_client_config: Arc<ClientConfig>,
    local_addr: SocketAddr,
) {
    loop {
        if let Err((msg, e)) = connection(
            server_addr(),
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
        tokio::time::delay_for(Duration::from_millis(CONN_LOOP_SLEEP)).await;
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
    let s2c = tokio::io::copy(&mut sr, &mut cw);
    let c2s = tokio::io::copy(&mut cr, &mut sw);
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
