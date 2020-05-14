use std::net::SocketAddrV4;
use futures::channel::oneshot::{channel, Sender, Receiver};
use futures::future::{self, Either, Future, FutureExt};
use tokio::net::TcpStream;
use std::io::Error as IoError;
use tokio::time::timeout as tokiotimeout;
use std::time::Duration;
use rustls::{RootCertStore, ClientConfig};
use tokio_rustls::TlsConnector;
use std::sync::Arc;
use tokio_rustls::client::TlsStream;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::prelude::*;
use crate::messaging::{Message, HelloV0, MessageError};

const EBBFLOW_DNS: &str = "s.ebbflow.io";
const LONG_TIMEOUT: Duration = Duration::from_secs(3);
const KILL_ACTIVE_DELAY: Duration = Duration::from_secs(60);

async fn ebbflow_addr() -> SocketAddrV4 {
    todo!()
}

fn ebbflow_dns() -> webpki::DNSName {
    webpki::DNSNameRef::try_from_ascii_str(EBBFLOW_DNS).unwrap().to_owned()
}

fn load_roots() -> Result<RootCertStore, ConnectionError> {
    match rustls_native_certs::load_native_certs() {
       rustls_native_certs::PartialResult::Ok(rcs) => Ok(rcs),
       rustls_native_certs::PartialResult::Err((Some(rcs), _)) => Ok(rcs),
       _ => Err(ConnectionError::RootStoreLoad)
    }
}

#[derive(Debug)]
enum ConnectionError {
    Io(IoError),
    Timeout(&'static str),
    Messaging(MessageError),
    RootStoreLoad,
}

impl From<MessageError> for ConnectionError {
    fn from(v: MessageError) -> Self {
        ConnectionError::Messaging(v)
    }
}

impl From<IoError> for ConnectionError {
    fn from(v: IoError) -> Self {
        ConnectionError::Io(v)
    }
}

pub struct EndpointConnectionArgs {
    endpoint: String,
    key: String,
    local_addr: SocketAddrV4,
    ctype: EndpointConnectionType,
    ebbflow_addr: SocketAddrV4,
    ebbflow_dns: webpki::DNSName,
}

pub enum EndpointConnectionType {
    Ssh,
    Tls,
}

pub struct EndpointConnection {
    signalsender: Option<Sender<()>>,
}

impl EndpointConnection {
    pub fn new(args: EndpointConnectionArgs) -> Self {
        let (s, r) = channel();

        tokio::spawn(run_connection(r, args));

        Self {
            signalsender: Some(s),
        }
    }

    pub fn stop(&mut self) {
        if let Some(sender) = self.signalsender.take() {
            let _ = sender.send(());
        }
    }
}

async fn run_connection(receiver: Receiver<()>, args: EndpointConnectionArgs) {
    match run_connection_fallible(receiver, &args).await {
        Ok(()) => {

        }
        Err(_e) => {

        }
    }
}

async fn run_connection_fallible(receiver: Receiver<()>, args: &EndpointConnectionArgs) -> Result<(), ConnectionError> {
    // Connect to Ebbflow
    let mut tlsstream = connect_ebbflow(args).await?;
    
    // Say Hello
    let hello = create_hello(args)?;
    tlsstream.write_all(&hello[..]).await?;
    tlsstream.flush().await?;

    let mut initial_buf = [0; 1024];

    // Await Connection
    let (n, receiver) = match futures::future::select(
        receiver,
       tlsstream.read(&mut initial_buf[..]),
    ).await {
        Either::Left((_, readf)) => {
            drop(readf);
            let (tcpstream, _) = tlsstream.into_inner();
            tcpstream.shutdown(std::net::Shutdown::Both)?;
            return Ok(());
        }
        Either::Right((readresult, r)) => {
            (readresult?, r)
        }
    };

    // We have some bytes and will proxy locally. First lets connect local
    let mut local = connect_local(args.local_addr.clone()).await?;

    let (proxyabortable, handle) = futures::future::abortable(Box::pin(async move {
        proxy(&mut local, &mut tlsstream, &initial_buf[0..n]).await
    }));

    match futures::future::select(
        receiver,
        proxyabortable,
    ).await {
        Either::Left((_, readf)) => {
            tokio::spawn(readf); // This lets the future continue running until we kill it
            tokio::time::delay_for(KILL_ACTIVE_DELAY).await;
            handle.abort();
            return Ok(());
        }
        Either::Right((proxyresult, _r)) => {
            match proxyresult {
                Ok(innerresult) => innerresult,
                Err(_) => {
                    // This seems unreachable? But let's handle it anyways..
                    Err(ConnectionError::Timeout("connection terminated"))
                }
            }
        }
    }
}

async fn connect_ebbflow(args: &EndpointConnectionArgs) -> Result<TlsStream<TcpStream>, ConnectionError> {
    let mut clientconfig = ClientConfig::new();
    clientconfig.root_store = load_roots()?;
    let connector = TlsConnector::from(Arc::new(clientconfig));
    let tcpstream = timeout(LONG_TIMEOUT, TcpStream::connect(args.ebbflow_addr.clone()), "connecting to ebbflow").await??;
    tcpstream.set_keepalive(Some(Duration::from_secs(1)))?;
    tcpstream.set_nodelay(true)?;
    Ok(connector.connect(args.ebbflow_dns.as_ref(), tcpstream).await?)
}

async fn connect_local(localaddr: SocketAddrV4) -> Result<TcpStream, ConnectionError> {
    let tcpstream = timeout(LONG_TIMEOUT, TcpStream::connect(localaddr), "connecting to local host").await??;
    tcpstream.set_keepalive(Some(Duration::from_secs(1)))?;
    tcpstream.set_nodelay(true)?;
    Ok(tcpstream)
}


fn create_hello(args: &EndpointConnectionArgs) -> Result<Vec<u8>, ConnectionError> {
    let t = match args.ctype {
        EndpointConnectionType::Ssh => crate::messaging::EndpointType::Ssh,
        EndpointConnectionType::Tls => crate::messaging::EndpointType::Tls,
    };
    let hello = HelloV0::new(args.key.clone(), t, args.endpoint.clone());
    let message = Message::HelloV0(hello);
    Ok(message.to_wire_message()?)
}

async fn proxy(local: &mut TcpStream, ebbflow: &mut TlsStream<TcpStream>, initialbuf: &[u8]) -> Result<(), ConnectionError> {
    let (mut localreader, mut localwriter) = tokio::io::split(local);
    let (mut ebbflowreader, mut ebbflowwriter) = tokio::io::split(ebbflow);

    localwriter.write_all(&initialbuf[..]).await?;
    localwriter.flush().await?;

    let local2ebb = Box::pin(async move {copy_bytes_ez(&mut localreader, &mut ebbflowwriter).await });
    let ebb2local = Box::pin(async move {copy_bytes_ez(&mut ebbflowreader, &mut localwriter).await });

    match futures::future::select(local2ebb, ebb2local).await {
        Either::Left((_server_read_res, _c2s_future)) => (),
        Either::Right((_client_read_res, _s2c_future)) => (),
    }

    todo!();
}

async fn copy_bytes_ez<R, W>(
    r: &mut R,
    w: &mut W,
) -> Result<(), ConnectionError>
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

async fn timeout<T>(duration: Duration, future: T, msg: &'static str) -> Result<T::Output, ConnectionError>
where
    T: Future,
{
    match tokiotimeout(duration, future).await {
        Ok(r) => Ok(r),
        Err(_) => Err(ConnectionError::Timeout(msg)),
    }
}
