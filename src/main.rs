#[macro_use]
extern crate log;
extern crate env_logger;

use async_std::net::TcpStream;
use std::time::Duration;
use futures::prelude::*;
use futures::io::{ReadHalf, WriteHalf};
use futures_rustls::client::TlsStream as ClientTlsStream;
use futures_rustls::rustls::{ClientConfig, ServerConfig, ServerSession, Session};
use futures_rustls::server::TlsStream as ServerTlsStream;
use futures_rustls::webpki::DNSNameRef;
use futures_rustls::{TlsAcceptor, TlsConnector};
use log::LevelFilter;
use std::net::SocketAddr;
use async_std::prelude::*;
use futures::io::{AsyncReadExt, AsyncWriteExt};
use std::sync::Arc;
use futures::future::Either;
use std::io::Error as IoError;
use futures::future::select;

use async_std::future::pending;

type Writer = Box<dyn AsyncWrite + Unpin + Send>;
type Reader = Box<dyn AsyncRead + Unpin + Send>;
type Clizzle = ClientTlsStream<TcpStream>; // Sick of typing it
type ReaderWriter = Box<dyn ReaderWriterTrait + Unpin + Send>;

trait ReaderWriterTrait : AsyncRead + AsyncWrite {}

const CONN_LOOP_SLEEP : u64 = 1000;

#[runtime::main]
async fn main() {
    env_logger::builder().filter_level(LevelFilter::Info).init();

    let server_addr = "127.0.0.1:7070".parse().expect("Unable to parse server address");
    let server_dns = DNSNameRef::try_from_ascii_str("test.ebbflow.io").expect("Unable to parse DNS name");

    let mut ccfg = ClientConfig::new();
    for cert in load_certs("../ebb/certs/myCA.pem") {
        ccfg.root_store.add(&cert).unwrap();
    }
    ccfg.set_single_client_cert(load_certs("../ebb/certs/test.crt"), load_private_key("../ebb/certs/test.key"));
    let server_client_config = Arc::new(ccfg);

    let local_addr = "127.0.0.1:8080".parse().expect("Unable to parse local address");

    for i in 0..3 {
        let c = server_client_config.clone();
        runtime::spawn(loopy(server_addr, server_dns, c, local_addr));
    }

    async_std::task::sleep(Duration::from_secs(60 * 60 * 60)).await;
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

async fn loopy(server_addr: SocketAddr, server_dns: DNSNameRef<'_>, server_client_config: Arc<ClientConfig>, local_addr: SocketAddr) {
    loop {
        if let Err(e) = connection(server_addr, server_dns, server_client_config.clone(), local_addr).await {
            warn!("Error from connection {:?}", e);
        }
        info!("Looping connection; Sleeping for {} millis before establishing a new connection", CONN_LOOP_SLEEP);
        async_std::task::sleep(Duration::from_millis(CONN_LOOP_SLEEP)).await;
    }
}

async fn connection(server_addr: SocketAddr, server_dns: DNSNameRef<'_>, server_client_config: Arc<ClientConfig>, local_addr: SocketAddr) -> Result<(), ConnError> {
    // Get a connection to Ebbflow
    debug!("Connecting to ebbflow");
    let (sr, sw) = connect_ebbflow(server_addr, server_dns, server_client_config).await?;

    // Get a connection to local
    debug!("Connecting to local addr");
    let (cr, cw) = connect_local(local_addr).await?;

    // Serve
    debug!("Proxying connection");
    proxy(sr, sw, cr, cw).await
}

async fn connect_ebbflow(addr: SocketAddr, dns: DNSNameRef<'_>, cfg: Arc<ClientConfig>) -> Result<(Reader, Writer), ConnError> {
    let stream = TcpStream::connect(addr).await?;

    let connector = TlsConnector::from(cfg);
    let tcpstream = connector.connect(dns, stream).await?;

    debug!("A connection has been established to ebbflow");

    let (r, w) = tcpstream.split();

    Ok((dyner_r(r), dyner_w(w)))
}

fn dyner_r(readhalf: ReadHalf<Clizzle>) -> Reader { Box::new(readhalf) }
fn dyner_w(writehalf: WriteHalf<Clizzle>) -> Writer { Box::new(writehalf) }
fn dyner_rs(readhalf: ReadHalf<TcpStream>) -> Reader { Box::new(readhalf) }
fn dyner_ws(writehalf: WriteHalf<TcpStream>) -> Writer { Box::new(writehalf) }

async fn connect_local(addr: SocketAddr) -> Result<(Reader, Writer), ConnError> {
    let tcpstream = TcpStream::connect(addr).await?;

    debug!("A connection has been established to the local server");

    let (r, w) = tcpstream.split();

    Ok((dyner_rs(r), dyner_ws(w)))
}

async fn proxy(mut sr: Reader, mut sw: Writer, mut cr: Reader, mut cw: Writer) -> Result<(), ConnError> {
    let s2c = sr.copy_into(&mut cw);
    let c2s = cr.copy_into(&mut sw);
    
    match select(s2c, c2s).await {
        Either::Left((server_read_res, c2s_future)) => {},
        Either::Right((client_read_res, s2c_future)) => {},
    }

    debug!("A proxied connection has terminated");

    Ok(())
}

use std::fs;
use std::io::BufReader;

pub fn load_certs(filename: &str) -> Vec<futures_rustls::rustls::Certificate> {
    let certfile = fs::File::open(filename).expect("cannot open certificate file");
    let mut reader = BufReader::new(certfile);
    futures_rustls::rustls::internal::pemfile::certs(&mut reader).unwrap()
}
pub fn load_private_key(filename: &str) -> futures_rustls::rustls::PrivateKey {
    let rsa_keys = {
        let keyfile = fs::File::open(filename).expect("cannot open private key file");
        let mut reader = BufReader::new(keyfile);
        futures_rustls::rustls::internal::pemfile::rsa_private_keys(&mut reader).expect("file contains invalid rsa private key")
    };

    let pkcs8_keys = {
        let keyfile = fs::File::open(filename).expect("cannot open private key file");
        let mut reader = BufReader::new(keyfile);
        futures_rustls::rustls::internal::pemfile::pkcs8_private_keys(&mut reader)
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