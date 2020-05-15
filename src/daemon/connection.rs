use crate::messaging::{
    HelloResponseIssue, HelloV0, Message, MessageError, StartTrafficResponseV0,
};
use crate::signal::SignalReceiver;
use futures::future::{Either, Future};
use std::io::Error as IoError;
use std::net::SocketAddrV4;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::prelude::*;
use tokio::time::timeout as tokiotimeout;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

const LONG_TIMEOUT: Duration = Duration::from_secs(3);
const SHORT_TIMEOUT: Duration = Duration::from_millis(500);
const KILL_ACTIVE_DELAY: Duration = Duration::from_secs(60);
const BAD_ERROR_DELAY: Duration = Duration::from_secs(30);
const MIN_EBBFLOW_ERROR_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug)]
enum ConnectionError {
    Io(IoError),
    Timeout(&'static str),
    Messaging(MessageError),
    UnexpectedMessage,
    Forbidden,
    NotFound,
    Shutdown,
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
    pub endpoint: String,
    pub key: String,
    pub local_addr: SocketAddrV4,
    pub ctype: EndpointConnectionType,
    pub connector: TlsConnector,
    pub ebbflow_addr: SocketAddrV4,
    pub ebbflow_dns: webpki::DNSName,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EndpointConnectionType {
    Ssh,
    Tls,
}

pub async fn run_connection(
    receiver: SignalReceiver,
    args: EndpointConnectionArgs,
    idle_permit: tokio::sync::OwnedSemaphorePermit,
) {
    let (ebbstream, localtcp) = match establish_ebbflow_connection(receiver.clone(), &args).await {
        Ok(x) => {
            trace!("A connection finished gracefully");
            // VVVVVV IMPORTANT: We drop the semaphore now that we have a connection!! This is because the semaphore counts
            // IDLE connections, not total.
            drop(idle_permit);
            x
        }
        Err(e) => {
            match e {
                ConnectionError::Forbidden | ConnectionError::NotFound => {
                    warn!(
                        "A connection for endpoint {} failed due to {:?}",
                        args.endpoint, e
                    );
                    info!("Bad error delay");
                    tokio::time::delay_for(BAD_ERROR_DELAY).await;
                    // We should sleep to avoid spamming, as NotFound and Forbidden errors
                    // are unlikely to be resolved anytime soon.
                }
                _ => {
                    info!(
                        "Failed to connect to Ebbflow for endpoint {} failed due to {:?}",
                        args.endpoint, e
                    );
                }
            }
            info!("Minimum Delay");
            tokio::time::delay_for(MIN_EBBFLOW_ERROR_DELAY).await;
            return;
        }
    };
    // TODO: Delay if we cannot connect locally

    match proxy_data(ebbstream, localtcp, &args, receiver).await {
        Ok(_) => {}
        Err(e) => {
            info!("error from proxy_data {:?}", e);
        }
    }
}

async fn establish_ebbflow_connection(
    mut receiver: SignalReceiver,
    args: &EndpointConnectionArgs,
) -> Result<(TlsStream<TcpStream>, TcpStream), ConnectionError> {
    // Connect to Ebbflow
    let mut tlsstream = connect_ebbflow(args).await?;

    let receiverfut = Box::pin(async move {
        receiver.wait().await;
    });

    // Say Hello
    let hello = create_hello(args)?;
    tlsstream.write_all(&hello[..]).await?;
    tlsstream.flush().await?;

    // Receive Response
    let message = tos(
        await_message(&mut tlsstream),
        "error waiting for hello response",
    )
    .await??;
    match message {
        Message::HelloResponseV0(hr) => match hr.issue {
            Some(HelloResponseIssue::Forbidden) => return Err(ConnectionError::Forbidden),
            Some(HelloResponseIssue::NotFound) => return Err(ConnectionError::NotFound),
            None => {} // Yay, we can continue!
        },
        _ => return Err(ConnectionError::UnexpectedMessage),
    }

    // Await Connection
    let mut stream = match futures::future::select(
        receiverfut,
        Box::pin(async move { await_traffic_start(tlsstream).await }),
    )
    .await
    {
        Either::Left((_, readf)) => {
            drop(readf);
            return Err(ConnectionError::Shutdown);
        }
        Either::Right((readresult, _r)) => readresult?,
    };

    // Traffic start, connect local real quick
    // We have some bytes and will proxy locally. First lets connect local
    let local = match connect_local(args.local_addr.clone()).await {
        Ok(localstream) => {
            trace!(
                "Connected to local address {:?} for endpoint {}",
                args.local_addr,
                args.endpoint
            );
            let response = starttrafficresponse(true)?;
            stream.write_all(&response[..]).await?;
            localstream
        }
        Err(e) => {
            warn!(
                "Error connecting to local address {:?} for endpoint {} {:?}",
                args.local_addr, args.endpoint, e
            );
            let response = starttrafficresponse(false)?;
            stream.write_all(&response[..]).await?;
            return Err(e);
        }
    };

    Ok((stream, local))
}

async fn proxy_data(
    mut tlsstream: TlsStream<TcpStream>,
    mut local: TcpStream,
    args: &EndpointConnectionArgs,
    mut receiver: SignalReceiver,
) -> Result<(), ConnectionError> {
    // Now we have both, let's create the proxy future, which we can hard-abort
    let (proxyabortable, handle) =
        futures::future::abortable(Box::pin(
            async move { proxy(&mut local, &mut tlsstream).await },
        ));

    match futures::future::select(
        Box::pin(async move { receiver.wait().await }),
        proxyabortable,
    )
    .await
    {
        // If the same receiver from above fires, let's sleep, then kill the connection
        Either::Left((_, readf)) => {
            tokio::spawn(readf); // This lets the future continue running until we kill it
            tokio::time::delay_for(KILL_ACTIVE_DELAY).await;
            handle.abort();
            return Ok(());
        }
        // The connection ran its course, but remember it was an abortable future so we need to look at that result first
        Either::Right((proxyresult, _r)) => {
            match proxyresult {
                Ok(innerresult) => innerresult,
                Err(_) => {
                    // This seems unreachable? The abort future should Err only if we called .abort which only happens if the Either::Left wins... But let's handle it anyways..
                    Err(ConnectionError::Timeout("unreachable segment, terminated"))
                }
            }
        }
    }
}

async fn connect_ebbflow(
    args: &EndpointConnectionArgs,
) -> Result<TlsStream<TcpStream>, ConnectionError> {
    let tcpstream = tol(
        TcpStream::connect(args.ebbflow_addr.clone()),
        "connecting to ebbflow",
    )
    .await??;
    tcpstream.set_keepalive(Some(Duration::from_secs(1)))?;
    tcpstream.set_nodelay(true)?;
    Ok(args
        .connector
        .connect(args.ebbflow_dns.as_ref(), tcpstream)
        .await?)
}

async fn await_message(tlsstream: &mut TlsStream<TcpStream>) -> Result<Message, ConnectionError> {
    let mut lenbuf: [u8; 4] = [0; 4];
    tlsstream.read_exact(&mut lenbuf[..]).await?;
    let len = u32::from_be_bytes(lenbuf) as usize;

    if len > 50 * 1024 {
        return Err(ConnectionError::Messaging(MessageError::Internal(
            "message too big",
        )));
    }

    let mut msgbuf = vec![0; len];
    tlsstream.read_exact(&mut msgbuf[..]).await?;
    Ok(Message::from_wire_without_the_length_prefix(&msgbuf[..])?)
}

async fn connect_local(localaddr: SocketAddrV4) -> Result<TcpStream, ConnectionError> {
    let tcpstream = tol(TcpStream::connect(localaddr), "connecting to local host").await??;
    tcpstream.set_keepalive(Some(Duration::from_secs(1)))?;
    tcpstream.set_nodelay(true)?;
    Ok(tcpstream)
}

async fn await_traffic_start(
    mut tlsstream: TlsStream<TcpStream>,
) -> Result<TlsStream<TcpStream>, ConnectionError> {
    let message = await_message(&mut tlsstream).await?;
    match message {
        Message::StartTrafficV0 => Ok(tlsstream),
        _ => return Err(ConnectionError::UnexpectedMessage),
    }
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

fn starttrafficresponse(good: bool) -> Result<Vec<u8>, ConnectionError> {
    let message = Message::StartTrafficResponseV0(StartTrafficResponseV0 {
        open_local_success_ready: good,
    });
    Ok(message.to_wire_message()?)
}

async fn proxy(
    local: &mut TcpStream,
    ebbflow: &mut TlsStream<TcpStream>,
) -> Result<(), ConnectionError> {
    let (mut localreader, mut localwriter) = tokio::io::split(local);
    let (mut ebbflowreader, mut ebbflowwriter) = tokio::io::split(ebbflow);

    let local2ebb =
        Box::pin(async move { copy_bytes_ez(&mut localreader, &mut ebbflowwriter).await });
    let ebb2local =
        Box::pin(async move { copy_bytes_ez(&mut ebbflowreader, &mut localwriter).await });

    match futures::future::select(local2ebb, ebb2local).await {
        Either::Left((_server_read_res, _c2s_future)) => (),
        Either::Right((_client_read_res, _s2c_future)) => (),
    }
    Ok(())
}

// ezpzlemonsqueezy
async fn copy_bytes_ez<R, W>(r: &mut R, w: &mut W) -> Result<(), ConnectionError>
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

async fn tol<T>(future: T, msg: &'static str) -> Result<T::Output, ConnectionError>
where
    T: Future,
{
    match tokiotimeout(LONG_TIMEOUT, future).await {
        Ok(r) => Ok(r),
        Err(_) => Err(ConnectionError::Timeout(msg)),
    }
}

async fn tos<T>(future: T, msg: &'static str) -> Result<T::Output, ConnectionError>
where
    T: Future,
{
    match tokiotimeout(SHORT_TIMEOUT, future).await {
        Ok(r) => Ok(r),
        Err(_) => Err(ConnectionError::Timeout(msg)),
    }
}
