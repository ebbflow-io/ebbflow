use crate::messaging::{
    HelloResponseIssue, HelloV0, Message, MessageError, StartTrafficResponseV0,
};
use crate::{messagequeue::MessageQueue, signal::SignalReceiver};
use futures::future::{Either, Future};
use rand::rngs::SmallRng;
use rand::Rng;
use rand::SeedableRng;
use std::io::Error as IoError;
use std::net::{SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::prelude::*;
use tokio::time::timeout as tokiotimeout;
use tokio::time::timeout;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

const LONG_TIMEOUT: Duration = Duration::from_secs(3);
const SHORT_TIMEOUT: Duration = Duration::from_millis(1_000);
const SUPER_SHORT_TIMEOUT: Duration = Duration::from_millis(300);
const KILL_ACTIVE_DELAY: Duration = Duration::from_secs(60);
const BAD_ERROR_DELAY: Duration = Duration::from_secs(55);
const MIN_EBBFLOW_ERROR_DELAY: Duration = Duration::from_secs(5);
const MAX_IDLE_CONNETION_TIME: Duration = Duration::from_secs(60 * 62); // at least an hour

#[derive(Debug)]
enum ConnectionError {
    Io(IoError),
    Timeout(&'static str),
    Messaging(MessageError),
    BadRequest,
    UnexpectedMessage,
    Forbidden,
    NotFound,
    Shutdown,
    Multiple(String),
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
    pub addrs: Vec<SocketAddr>,
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

/// Entry point for establishing an individual connection to Ebbflow, awaiting a connection, and then proxying data between the two.
pub async fn run_connection(
    receiver: SignalReceiver,
    args: EndpointConnectionArgs,
    idle_permit: tokio::sync::OwnedSemaphorePermit,
    meta: Arc<super::EndpointMeta>,
    message: Arc<MessageQueue>,
) {
    meta.add_idle();
    let getstreamresult = timeout(
        MAX_IDLE_CONNETION_TIME,
        establish_ebbflow_connection_and_await_traffic_signal(receiver.clone(), &args),
    )
    .await;
    // Important: Release any idle connections ASAP so we can start another connection
    meta.remove_idle();

    let stream = match getstreamresult {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            match e {
                ConnectionError::Forbidden
                | ConnectionError::NotFound
                | ConnectionError::BadRequest => {
                    let s = format!(
                        "A connection for endpoint {} failed due to {:?}",
                        args.endpoint, e
                    );
                    warn!("{}", s);
                    message.add_message(s);
                    trace!("Bad error delay");
                    jittersleep1p5(BAD_ERROR_DELAY).await;
                }
                _ => {
                    debug!(
                        "Failed to connect to Ebbflow for endpoint {} failed due to {:?}",
                        args.endpoint, e
                    );
                }
            }
            trace!("Minimum Delay");
            jittersleep1p5(MIN_EBBFLOW_ERROR_DELAY).await;

            return;
        }
        Err(_e) => {
            debug!(
                "Timed out connecting to Ebbflow {:?}",
                MAX_IDLE_CONNETION_TIME
            );
            return;
        }
    };
    drop(idle_permit);

    let (ebbstream, localtcp, _now) =
        match connect_local_with_ebbflow_communication(stream, &args, message).await {
            Ok(triple) => triple,
            Err(_e) => {
                trace!("Minimum Delay");
                jittersleep1p5(MIN_EBBFLOW_ERROR_DELAY).await;
                return;
            }
        };
    trace!(
        "Connection handshake complete and a local connection has been established, proxying data"
    );

    // We have two connections that are ready to be proxied, lesgo
    meta.add_active();
    let r = proxy_data(ebbstream, localtcp, &args, receiver).await;
    meta.remove_active();
    match r {
        Ok(_) => {}
        Err(e) => {
            debug!("error from proxy_data {:?}", e);
        }
    }
}

/// This waits for a connection to Ebbflow, and only returns with a valid connection to the local server.
///
/// Specifically, this will inform Ebbflow if the local connection is ready or not after it connects locally.
async fn establish_ebbflow_connection_and_await_traffic_signal(
    mut receiver: SignalReceiver,
    args: &EndpointConnectionArgs,
) -> Result<TlsStream<TcpStream>, ConnectionError> {
    // Connect to Ebbflow
    let mut tlsstream = connect_ebbflow(args).await?;

    let receiverfut = Box::pin(async move {
        receiver.wait().await;
        trace!("Receiver told to stop");
    });

    // Say Hello
    let hello = create_hello(args)?;
    tlsstream.write_all(&hello[..]).await?;
    tlsstream.flush().await?;

    // Receive Response, timed out as Ebbflow should be quick
    let message = tos(
        await_message(&mut tlsstream),
        "error waiting for hello response",
    )
    .await??;
    match message {
        Message::HelloResponseV0(hr) => match hr.issue {
            Some(HelloResponseIssue::Forbidden) => return Err(ConnectionError::Forbidden),
            Some(HelloResponseIssue::NotFound) => return Err(ConnectionError::NotFound),
            Some(HelloResponseIssue::BadRequest) => return Err(ConnectionError::BadRequest),
            None => {} // Yay, we can continue!
        },
        _ => return Err(ConnectionError::UnexpectedMessage),
    }
    debug!("Awaiting TrafficStart ({})", args.endpoint,);

    // Await Connection
    let stream = match futures::future::select(
        receiverfut,
        Box::pin(async move { await_traffic_start(tlsstream).await }), // No timeout, as the connection can be idle for a while
    )
    .await
    {
        Either::Left((_, readf)) => {
            trace!("Stopping connection await due to signal stop");
            drop(readf);
            return Err(ConnectionError::Shutdown);
        }
        Either::Right((readresult, _r)) => readresult?,
    };
    Ok(stream)
}

async fn connect_local_with_ebbflow_communication(
    mut stream: TlsStream<TcpStream>,
    args: &EndpointConnectionArgs,
    message: Arc<MessageQueue>,
) -> Result<(TlsStream<TcpStream>, TcpStream, Instant), ConnectionError> {
    let now = Instant::now();
    // Traffic start, connect local real quick
    let local = match connect_local(&args.addrs).await {
        Ok(localstream) => {
            trace!(
                "Connected to local addr {:?} for {}",
                args.addrs,
                args.endpoint
            );
            let response = starttrafficresponse(true)?;
            stream.write_all(&response[..]).await?;
            localstream
        }
        Err(e) => {
            let s = format!(
                "ERROR: Received traffic but could not connect to local host addr {:?} for {} {:?}",
                args.addrs, args.endpoint, e
            );
            warn!("{}", s);
            message.add_message(s);
            let response = starttrafficresponse(false)?;
            stream.write_all(&response[..]).await?;
            return Err(e);
        }
    };

    Ok((stream, local, now))
}

async fn proxy_data(
    mut tlsstream: TlsStream<TcpStream>,
    mut local: TcpStream,
    _args: &EndpointConnectionArgs,
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
            jittersleep1p5(KILL_ACTIVE_DELAY).await;
            handle.abort();
            Ok(())
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
        TcpStream::connect(args.ebbflow_addr),
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

    if len > 200 * 1024 {
        return Err(ConnectionError::Messaging(MessageError::Internal(
            "message too big safeguard",
        )));
    }

    let mut msgbuf = vec![0; len];
    tlsstream.read_exact(&mut msgbuf[..]).await?;
    Ok(Message::from_wire_without_the_length_prefix(&msgbuf[..])?)
}

async fn connect_local(addrs: &[SocketAddr]) -> Result<TcpStream, ConnectionError> {
    let mut errors = Vec::new();
    for a in addrs {
        match toss(TcpStream::connect(a), "connecting to local host").await {
            Ok(Ok(tcpstream)) => {
                tcpstream.set_keepalive(Some(Duration::from_secs(1)))?;
                tcpstream.set_nodelay(true)?;
                return Ok(tcpstream);
            }
            Ok(Err(e)) => {
                errors.push((a.clone(), e.into()));
            }
            Err(e) => {
                errors.push((a.clone(), e));
            }
        }
    }
    Err(ConnectionError::Multiple(format!(
        "multiple {}",
        errors
            .iter()
            .map(|(a, e)| format!("{:?} - {:?}", a, e))
            .collect::<Vec<String>>()
            .join(", ")
    )))
}

async fn await_traffic_start(
    mut tlsstream: TlsStream<TcpStream>,
) -> Result<TlsStream<TcpStream>, ConnectionError> {
    let message = await_message(&mut tlsstream).await?;
    match message {
        Message::StartTrafficV0 => Ok(tlsstream),
        _ => Err(ConnectionError::UnexpectedMessage),
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

/// Actually transfer the bytes between the two parties
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
    let mut buf = vec![0; 10 * 1024];
    let mut first = true;
    loop {
        let n = r.read(&mut buf[0..]).await?;
        if first {
            first = false;
        }

        if n == 0 {
            return Ok(());
        }
        w.write_all(&buf[0..n]).await?;
        w.flush().await?;
    }
}

/// long timeout
async fn tol<T>(future: T, msg: &'static str) -> Result<T::Output, ConnectionError>
where
    T: Future,
{
    match tokiotimeout(LONG_TIMEOUT, future).await {
        Ok(r) => Ok(r),
        Err(_) => Err(ConnectionError::Timeout(msg)),
    }
}

/// super short timeout
async fn toss<T>(future: T, msg: &'static str) -> Result<T::Output, ConnectionError>
where
    T: Future,
{
    match tokiotimeout(SUPER_SHORT_TIMEOUT, future).await {
        Ok(r) => Ok(r),
        Err(_) => Err(ConnectionError::Timeout(msg)),
    }
}

/// short timeout
async fn tos<T>(future: T, msg: &'static str) -> Result<T::Output, ConnectionError>
where
    T: Future,
{
    match tokiotimeout(SHORT_TIMEOUT, future).await {
        Ok(r) => Ok(r),
        Err(_) => Err(ConnectionError::Timeout(msg)),
    }
}

// Sleeps somewhere between 1 and 1.5 of the `dur`
async fn jittersleep1p5(dur: Duration) {
    let mut small_rng = SmallRng::from_entropy();
    let max = dur.mul_f32(1.5);
    let millis = small_rng.gen_range(dur.as_millis(), max.as_millis());
    trace!("jittersleep {}ms", millis);
    tokio::time::delay_for(Duration::from_millis(millis as u64)).await;
}
