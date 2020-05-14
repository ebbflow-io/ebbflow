//! This is a mock version of Ebbflow to be used for testing
use tokio_rustls::TlsAcceptor;
use tokio::net::TcpListener;
use std::io;
use rustls::ServerConfig;
use tokio::prelude::*;
use rustls::RootCertStore;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use rustls::{Certificate, PrivateKey};
use std::fs;
use std::io::BufReader;
use std::sync::Arc;
use tokio_rustls::server::TlsStream;
use tokio::net::TcpStream;
use parking_lot::Mutex;

pub async fn listen_and_process(port_for_customers: usize, port_for_tested_client: usize) -> Result<(), io::Error> {
    let tested_clients = Arc::new(Mutex::new(Vec::new()));
    let mut scfg = ServerConfig::new(rustls::NoClientAuth::new());
    scfg.set_single_cert(load_certs("tests/certs/test.crt"), load_private_key("tests/certs/test.key")).unwrap();
    let scfg = Arc::new(scfg);
    let acceptor = TlsAcceptor::from(scfg.clone());
    let mut listener = TcpListener::bind(format!("127.0.0.1:{}", port_for_tested_client)).await?;

    let tc = tested_clients.clone();
    tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let tlsstream = acceptor.accept(socket).await.unwrap();

            // receive message

            // send message

            tc.lock().push(tlsstream);
        }
    });

    tokio::spawn(async move {
        let mut listener = TcpListener::bind(format!("127.0.0.1:{}", port_for_customers)).await.unwrap();

        let tc = tested_clients.clone();
        loop {
            let (socket, _) = listener.accept().await.unwrap();

            let clienttestedstream = tc.lock().pop().unwrap();

            // write message

            tokio::spawn(copyezcopy(clienttestedstream, socket));
        }
    });
    futures::future::pending::<()>().await;
    Ok(())
}

pub async fn copyezcopy(stream1: TlsStream<TcpStream>, stream2: TcpStream) {
    let (mut s1r, mut s1w) = tokio::io::split(stream1);
    let (mut s2r, mut s2w) = tokio::io::split(stream2);

    tokio::spawn(async move {
        copy_bytes_ez(&mut s1r, &mut s2w).await.unwrap();
    });
    tokio::spawn(async move {
        copy_bytes_ez(&mut s2r, &mut s1w).await.unwrap();
    });
}

async fn copy_bytes_ez<R, W>(
    r: &mut R,
    w: &mut W,
) -> Result<(), std::io::Error>
where 
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
 {
    let mut buf = [0; 10 * 1024];

    loop {
        let n = r.read(&mut buf[0..]).await?;

        if n == 0 {
            return Ok(());
        }
        w.write_all(&buf[0..n]).await?;
        w.flush().await?;
    }
}

pub fn load_root() -> RootCertStore {
    let mut store = RootCertStore::empty();
    for cert in load_certs("tests/certs/myCA.pem") {
        store.add(&cert).unwrap();
    }
    store
}

/// PANICS!!!!
pub fn load_certs(filename: &str) -> Vec<Certificate> {
    let certfile = fs::File::open(filename).expect("cannot open certificate file");
    let mut reader = BufReader::new(certfile);
    rustls::internal::pemfile::certs(&mut reader).unwrap()
}

/// PANICS!!
pub fn load_private_key(filename: &str) -> PrivateKey {
    let rsa_keys = {
        let keyfile = fs::File::open(filename).expect("cannot open private key file");
        let mut reader = BufReader::new(keyfile);
        rustls::internal::pemfile::rsa_private_keys(&mut reader)
            .expect("file contains invalid rsa private key")
    };

    let pkcs8_keys = {
        let keyfile = fs::File::open(filename).expect("cannot open private key file");
        let mut reader = BufReader::new(keyfile);
        rustls::internal::pemfile::pkcs8_private_keys(&mut reader)
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