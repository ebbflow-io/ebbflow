pub mod connection;
pub mod health;

use crate::daemon::connection::{run_connection, EndpointConnectionArgs, EndpointConnectionType};
use crate::dns::DnsResolver;
use crate::{
    certs::ROOTS,
    config::{ConcreteHealthCheck, HealthCheck, HealthCheckType},
    messagequeue::MessageQueue,
    signal::{SignalReceiver, SignalSender},
};
use futures::future::select;
use futures::future::Either;
use health::HealthMaster;
use parking_lot::Mutex;
use rand::rngs::SmallRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use rustls::{ClientConfig, RootCertStore};
use std::net::{SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::{collections::VecDeque, time::Duration};
use tokio::{
    sync::Semaphore,
    time::{delay_for, timeout},
};
use tokio_rustls::TlsConnector;

const EBBFLOW_DNS: &str = "s.ebbflow.io";
const EBBFLOW_PORT: u16 = 443;
const MAX_IDLE: usize = 1000;

pub struct SharedInfo {
    dns: DnsResolver,
    key: Mutex<Option<String>>,
    roots: RootCertStore,
    hardcoded_ebbflow_addr: Option<SocketAddrV4>,
    hardcoded_ebbflow_dns: Option<String>,
}

impl SharedInfo {
    pub async fn new() -> Result<Self, ()> {
        Self::innernew(None, None, ROOTS.clone()).await
    }

    pub async fn new_with_ebbflow_overrides(
        hardcoded_ebbflow_addr: SocketAddrV4,
        hardcoded_ebbflow_dns: String,
        roots: RootCertStore,
    ) -> Result<Self, ()> {
        Self::innernew(
            Some(hardcoded_ebbflow_addr),
            Some(hardcoded_ebbflow_dns),
            roots,
        )
        .await
    }

    async fn innernew(
        overriddenmaybe: Option<SocketAddrV4>,
        overridedns: Option<String>,
        roots: RootCertStore,
    ) -> Result<Self, ()> {
        Ok(Self {
            dns: DnsResolver::new().await?,
            key: Mutex::new(None),
            roots,
            hardcoded_ebbflow_addr: overriddenmaybe,
            hardcoded_ebbflow_dns: overridedns,
        })
    }

    pub fn update_key(&self, newkey: String) {
        let mut key = self.key.lock();
        *key = Some(newkey);
    }

    pub fn key(&self) -> Option<String> {
        self.key.lock().clone()
    }

    pub fn roots(&self) -> RootCertStore {
        self.roots.clone()
    }

    pub async fn ebbflow_addr(&self) -> SocketAddrV4 {
        if let Some(overridden) = self.hardcoded_ebbflow_addr {
            return overridden;
        }
        let ips = self.dns.ips(EBBFLOW_DNS).await.unwrap_or_else(|_| {
            vec![
                "75.2.87.195".parse().unwrap(),
                "99.83.181.168".parse().unwrap(),
                "34.120.207.167".parse().unwrap(),
            ]
        }); // TODO: Update fallbacks

        let mut small_rng = SmallRng::from_entropy();
        let chosen = ips[..].choose(&mut small_rng);
        SocketAddrV4::new(*chosen.unwrap(), EBBFLOW_PORT)
    }

    pub fn ebbflow_dns(&self) -> webpki::DNSName {
        if let Some(overridden) = &self.hardcoded_ebbflow_dns {
            webpki::DNSNameRef::try_from_ascii_str(&overridden)
                .unwrap()
                .to_owned()
        } else {
            webpki::DNSNameRef::try_from_ascii_str(EBBFLOW_DNS)
                .unwrap()
                .to_owned()
        }
    }
}

#[derive(Debug, Clone)]
pub struct EndpointArgs {
    pub ctype: EndpointConnectionType,
    pub idleconns: usize,
    pub maxconns: usize,
    pub endpoint: String,
    pub port: u16,
    pub message_queue: Arc<MessageQueue>,
    pub healthcheck: Option<HealthCheck>,
}

pub struct EndpointMeta {
    idle: AtomicUsize,
    active: AtomicUsize,
    stopper: SignalSender,
    healthdata: Option<Arc<HealthMaster>>,
}
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HealthOverall {
    NOT_CONFIGURED,
    HEALTHY(VecDeque<(bool, u128)>),
    UNHEALTHY(VecDeque<(bool, u128)>),
}

impl EndpointMeta {
    pub fn new(stopper: SignalSender, hm: Option<Arc<HealthMaster>>) -> Self {
        Self {
            idle: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            healthdata: hm,
            stopper,
        }
    }

    pub fn num_active(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    pub fn num_idle(&self) -> usize {
        self.idle.load(Ordering::SeqCst)
    }

    pub fn stop(&self) {
        self.stopper.send_signal();
    }

    pub fn health(&self) -> HealthOverall {
        match &self.healthdata {
            None => HealthOverall::NOT_CONFIGURED,
            Some(hm) => {
                let (status, datapoints) = hm.data();
                if status {
                    HealthOverall::HEALTHY(datapoints)
                } else {
                    HealthOverall::UNHEALTHY(datapoints)
                }
            }
        }
    }

    fn add_idle(&self) {
        self.idle.fetch_add(1, Ordering::SeqCst);
    }

    fn remove_idle(&self) {
        self.idle.fetch_sub(1, Ordering::SeqCst);
    }

    fn add_active(&self) {
        self.active.fetch_add(1, Ordering::SeqCst);
    }

    fn remove_active(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// This runs this endpoint. To stop it, SEND THE SIGNAL.
pub async fn spawn_endpoint(info: Arc<SharedInfo>, mut args: EndpointArgs) -> Arc<EndpointMeta> {
    // This sender and receiver are the 'master' sender and receivers that are sent
    // to us when 'enabled' or 'disabled' is set on the endpoint in the config
    let sender = SignalSender::new();
    let receiver = sender.new_receiver();

    let maybe_hc = match &args.healthcheck {
        None => None,
        Some(hc) => {
            let chc = ConcreteHealthCheck::new(args.port, &hc);
            Some(Arc::new(HealthMaster::new(chc)))
        }
    };

    let meta = Arc::new(EndpointMeta::new(sender, maybe_hc.clone()));
    let metac1 = meta.clone();
    let endpoint = args.endpoint.clone();

    let message_queue = args.message_queue.clone();

    args.idleconns = std::cmp::min(args.idleconns, MAX_IDLE);

    let m = format!("Endpoint {} is enabled", endpoint);
    info!("{}", m);
    message_queue.add_message(m);

    let conn_addrs = get_addrs(args.port, args.message_queue.clone()).await;
    let hc_info = match maybe_hc {
        None => None,
        Some(hc) => Some((
            hc.clone(),
            get_addrs(hc.cfg.port, args.message_queue.clone()).await,
        )),
    };

    let e = endpoint.clone();
    tokio::spawn(async move {
        loop {
            // See if we go unhealthy, or the main receiver is done first.
            let mut r = receiver.clone();
            match select(
                Box::pin(async {
                    await_healthy(&hc_info, &e, &message_queue).await;
                }),
                Box::pin(async move { r.wait().await }),
            )
            .await
            {
                // healthy!
                Either::Left(_) => (),
                // We were told to stop, so don't loop
                Either::Right(_) => {
                    message_queue
                        .add_message(format!("Endpoint {} was disabled, shutting down", endpoint));
                    break;
                }
            }

            let m = format!("Endpoint {} starting up", e);
            info!("{}", m);
            message_queue.add_message(m);
            let healthy_connection_sender = SignalSender::new();
            let mut healthy_connection_receiver_conns1 = healthy_connection_sender.new_receiver();
            let healthy_connection_receiver_conns2 = healthy_connection_sender.new_receiver();

            let i = info.clone();
            let m = metac1.clone();
            let a = args.clone();
            let ca = conn_addrs.clone();
            let ee = e.clone();
            tokio::spawn(async move {
                match select(
                    Box::pin(async move { healthy_connection_receiver_conns1.wait().await }),
                    Box::pin(async move {
                        inner_run_endpoint(i, a.clone(), healthy_connection_receiver_conns2, m, ca)
                            .await
                    }),
                )
                .await
                {
                    Either::Left(_) => debug!("Endpoint {} is shutting down", &ee),
                    Either::Right(_) => info!("Unreachable? inner_run_endpoint finished"),
                }
            });

            // See if we go unhealthy, or the main receiver is done first.
            let mut r = receiver.clone();
            match select(
                Box::pin(async {
                    await_unhealthy(&hc_info, &endpoint, &message_queue).await;
                }),
                Box::pin(async move { r.wait().await }),
            )
            .await
            {
                // Unhealthy!
                Either::Left(_) => (),
                // We were told to stop, so don't loop
                Either::Right(_) => {
                    message_queue
                        .add_message(format!("Endpoint {} was disabled, shutting down", endpoint));
                    break;
                }
            }
            // Kill the connections, we are unhealthy
            healthy_connection_sender.send_signal();

            let m = format!("Endpoint {} shutting down", e);
            info!("{}", m);
            message_queue.add_message(m);
        }
        let m = format!("Endpoint {} is disabled", endpoint);
        info!("{}", m);
        message_queue.add_message(m);
    });

    meta
}

async fn await_healthy(
    hc: &Option<(Arc<HealthMaster>, Vec<SocketAddr>)>,
    e: &str,
    mq: &Arc<MessageQueue>,
) {
    match hc {
        // Start healthy if no healthcheck
        None => (),
        Some((hc, addrs)) => loop {
            let r = healthcheck(&hc.cfg, addrs, e).await;

            if hc.report_check_result(r) {
                let m = format!("Endpoint {} considered healthy", e);
                info!("{}", m);
                mq.add_message(m);
                return;
            }

            delay_for(Duration::from_secs(hc.cfg.frequency_secs as u64)).await;
        },
    }
}

async fn await_unhealthy(
    hc: &Option<(Arc<HealthMaster>, Vec<SocketAddr>)>,
    e: &str,
    mq: &Arc<MessageQueue>,
) {
    match hc {
        // Never consider unhealthy if no healthcheck
        None => futures::future::pending::<()>().await,
        Some((hc, addrs)) => loop {
            let r = healthcheck(&hc.cfg, addrs, e).await;

            if !hc.report_check_result(r) {
                let m = format!("Endpoint {} considered unhealthy", e);
                info!("{}", m);
                mq.add_message(m);
                return;
            }

            delay_for(Duration::from_secs(hc.cfg.frequency_secs as u64)).await;
        },
    }
}

// Checks both ipv4 and ipv6 addr, if can connect returns true
async fn healthcheck(hc: &ConcreteHealthCheck, addrs: &Vec<SocketAddr>, e: &str) -> bool {
    match timeout(Duration::from_secs(1), inner_healthcheck(hc, addrs)).await {
        Ok(r) => {
            debug!("{} health check result: {}", e, r);
            r
        }
        Err(_) => {
            debug!("{} health check timed out and was considered a failure", e);
            false
        }
    }
}

async fn inner_healthcheck(hc: &ConcreteHealthCheck, addrs: &Vec<SocketAddr>) -> bool {
    match hc.r#type {
        HealthCheckType::TCP => {
            for addr in addrs {
                if tokio::net::TcpStream::connect(addr).await.is_ok() {
                    return true;
                }
            }
            false
        }
    }
}

async fn get_addrs(port: u16, mq: Arc<MessageQueue>) -> Vec<SocketAddr> {
    loop {
        match tokio::net::lookup_host(format!("localhost:{}", port)).await {
            Ok(i) => {
                let mut addrs: Vec<SocketAddr> = i.collect();
                // prioritize ipv4 addrs
                addrs.sort_by(|l, r| match (l.is_ipv4(), r.is_ipv4()) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => std::cmp::Ordering::Equal,
                });
                break addrs;
            }
            Err(e) => {
                let msg = format!("Extremely unexpected error, could not resolve DNS of localhost, will try again in a second {:?}", e);
                error!("{}", msg);
                mq.add_message(msg);
                tokio::time::delay_for(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn inner_run_endpoint(
    info: Arc<SharedInfo>,
    args: EndpointArgs,
    receiver: SignalReceiver,
    meta: Arc<EndpointMeta>,
    addrs: Vec<SocketAddr>,
) {
    let mut ccfg = ClientConfig::new();
    ccfg.root_store = info.roots();
    let idlesem = Arc::new(Semaphore::new(args.idleconns));
    let maxsem = Arc::new(Semaphore::new(args.maxconns));
    let ccfg = Arc::new(ccfg);

    loop {
        let idlesemc = idlesem.clone();
        let maxsemc = maxsem.clone();

        // We can never have more than MAX permits out, we must have one to have a connection.
        let maxpermit = maxsemc.acquire_owned().await;
        trace!(
            "acquired max permit i{} a{}",
            meta.num_idle(),
            meta.num_active()
        );
        // Once we are OK with our max, we must have an IDLE connection available
        let idlepermit = idlesemc.acquire_owned().await;
        trace!("acquired idle permit");

        // We have a permit, start a connection
        let receiverc = receiver.clone();
        let messageq = args.message_queue.clone();
        let args = create_args(&info, &args, &addrs[..], ccfg.clone()).await;
        debug!(
            "Creating new connection runner for {} (localaddr: {:?})",
            args.endpoint, args.addrs
        );
        let m = meta.clone();
        trace!(
            "ebbflow addrs {:?} dns {:?}",
            args.ebbflow_addr,
            args.ebbflow_dns
        );
        tokio::spawn(async move {
            run_connection(receiverc, args, idlepermit, m.clone(), messageq).await;
            debug!("Connection ended i{} a{}", m.num_idle(), m.num_active());
            drop(maxpermit);
        });
    }
}

async fn create_args(
    info: &Arc<SharedInfo>,
    args: &EndpointArgs,
    addrs: &[SocketAddr],
    ccfg: Arc<ClientConfig>,
) -> EndpointConnectionArgs {
    let connector = TlsConnector::from(ccfg);

    EndpointConnectionArgs {
        endpoint: args.endpoint.clone(),
        key: info.key().unwrap_or_else(|| "unset".to_string()),
        addrs: addrs.to_vec(),
        ctype: args.ctype,
        ebbflow_addr: info.ebbflow_addr().await,
        ebbflow_dns: info.ebbflow_dns(),
        connector,
    }
}
