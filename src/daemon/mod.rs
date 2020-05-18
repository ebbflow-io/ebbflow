pub mod connection;

use crate::daemon::connection::{run_connection, EndpointConnectionArgs, EndpointConnectionType};
use crate::dns::DnsResolver;
use crate::signal::SignalReceiver;
use futures::future::select;
use futures::future::Either;
use parking_lot::Mutex;
use rand::rngs::SmallRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use rustls::{ClientConfig, RootCertStore};
use std::net::SocketAddrV4;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_rustls::TlsConnector;

const EBBFLOW_DNS: &str = "s.preview.ebbflow.io"; // TODO obvi, lets use use a trusted cert
const EBBFLOW_PORT: u16 = 7070;

pub struct SharedInfo {
    dns: DnsResolver,
    key: Mutex<Option<String>>,
    roots: Mutex<RootCertStore>,
    hardcoded_ebbflow_addr: Option<SocketAddrV4>,
    hostname: String,
}

impl SharedInfo {
    pub async fn new(roots: RootCertStore, hostname: String) -> Result<Self, ()> {
        Self::innernew(None, roots, hostname).await
    }

    pub async fn new_with_ebbflow_overrides(
        hardcoded_ebbflow_addr: SocketAddrV4,
        roots: RootCertStore,
        hostname: String,
    ) -> Result<Self, ()> {
        Self::innernew(Some(hardcoded_ebbflow_addr), roots, hostname).await
    }

    async fn innernew(
        overriddenmaybe: Option<SocketAddrV4>,
        roots: RootCertStore,
        hostname: String,
    ) -> Result<Self, ()> {
        Ok(Self {
            dns: DnsResolver::new().await?,
            key: Mutex::new(None),
            roots: Mutex::new(roots),
            hardcoded_ebbflow_addr: overriddenmaybe,
            hostname,
        })
    }

    pub fn hostname(&self) -> String {
        self.hostname.clone()
    }

    pub fn update_key(&self, newkey: String) {
        let mut key = self.key.lock();
        *key = Some(newkey);
    }

    pub fn key(&self) -> Option<String> {
        self.key.lock().clone()
    }

    pub fn roots(&self) -> RootCertStore {
        self.roots.lock().clone()
    }

    pub fn update_roots(&self, newroots: RootCertStore) {
        let mut roots = self.roots.lock();
        *roots = newroots;
    }

    pub async fn ebbflow_addr(&self) -> SocketAddrV4 {
        if let Some(overridden) = self.hardcoded_ebbflow_addr {
            return overridden.clone();
        }
        let ips = self
            .dns
            .ips(EBBFLOW_DNS)
            .await
            .unwrap_or_else(|_| Vec::new()); // TODO: Return fallback IPs here


        let mut small_rng = SmallRng::from_entropy();
        let chosen = ips[..].choose(&mut small_rng);
        SocketAddrV4::new(chosen.unwrap().clone(), EBBFLOW_PORT)
    }

    pub fn ebbflow_dns(&self) -> webpki::DNSName {
        webpki::DNSNameRef::try_from_ascii_str(EBBFLOW_DNS)
            .unwrap()
            .to_owned()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndpointArgs {
    pub ctype: EndpointConnectionType,
    pub idleconns: usize,
    pub maxconns: usize,
    pub endpoint: String,
    pub local_addr: SocketAddrV4,
}

/// This runs this endpoint. To stop it, SEND THE SIGNAL
pub async fn spawn_endpoint(info: Arc<SharedInfo>, args: EndpointArgs, receiver: SignalReceiver) {
    let mut ourreceiver = receiver.clone();
    tokio::spawn(async move {
        match select(
            Box::pin(async move { ourreceiver.wait().await }),
            Box::pin(async move { inner_run_endpoint(info, args, receiver).await }),
        )
        .await
        {
            Either::Left(_) => debug!("Endpoint runner told to stop"),
            Either::Right(_) => debug!("Unreachable? inner_run_endpoint finished"),
        }
    });
}

async fn inner_run_endpoint(info: Arc<SharedInfo>, args: EndpointArgs, receiver: SignalReceiver) {
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
        trace!("acquired max permit");
        // Once we are OK with our max, we must have an IDLE connection available
        let idlepermit = idlesemc.acquire_owned().await;
        trace!("acquired idle permit");

        // We have a permit, start a connection
        let receiverc = receiver.clone();
        let args = create_args(&info, &args, ccfg.clone()).await;
        debug!(
            "A new connection to ebbflow will be established for endpoint {} (localaddr: {:?})",
            args.endpoint, args.local_addr
        );
        trace!("ebbflow addrs {:?} dns {:?}", args.ebbflow_addr, args.ebbflow_dns);
        tokio::spawn(async move {
            run_connection(receiverc, args, idlepermit).await;
            drop(maxpermit);
        });
    }
}

async fn create_args(
    info: &Arc<SharedInfo>,
    args: &EndpointArgs,
    ccfg: Arc<ClientConfig>,
) -> EndpointConnectionArgs {
    let connector = TlsConnector::from(ccfg);

    EndpointConnectionArgs {
        endpoint: args.endpoint.clone(),
        key: info.key().unwrap_or_else(|| "unset".to_string()),
        local_addr: args.local_addr.clone(),
        ctype: args.ctype,
        ebbflow_addr: info.ebbflow_addr().await,
        ebbflow_dns: info.ebbflow_dns(),
        connector,
    }
}
