use std::net::{IpAddr, Ipv4Addr};
use trust_dns_resolver::config::NameServerConfigGroup;
use trust_dns_resolver::config::ResolverConfig;
use trust_dns_resolver::config::ResolverOpts;
use trust_dns_resolver::TokioAsyncResolver;

pub struct DnsResolver {
    trust: TokioAsyncResolver,
}
use std::time::Duration;
const TIMEOUT: Duration = Duration::from_secs(1);
const TTL: Duration = Duration::from_secs(60 * 60);

impl DnsResolver {
    pub async fn new() -> Result<Self, ()> {
        let mut opts = ResolverOpts::default();
        opts.positive_max_ttl = Some(TTL);
        opts.negative_max_ttl = Some(TTL);

        let group = NameServerConfigGroup::from_ips_clear(
            &[
                IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
                IpAddr::V4(Ipv4Addr::new(1, 0, 0, 1)),
                IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9)),
                IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
                IpAddr::V4(Ipv4Addr::new(8, 8, 4, 4)),
            ],
            53,
        );
        let config = ResolverConfig::from_parts(None, vec![], group);

        let r = TokioAsyncResolver::tokio(config, opts)
            .await
            .map_err(|_e| ())?;
        Ok(Self { trust: r })
    }

    pub async fn ips(&self, domain: &str) -> Result<Vec<Ipv4Addr>, ()> {
        let ips = match tokio::time::timeout(TIMEOUT, self.trust.ipv4_lookup(domain))
            .await
            .map_err(|_| ())?
        {
            Err(e) => {
                if let trust_dns_resolver::error::ResolveErrorKind::NoRecordsFound {
                    query: _,
                    valid_until: _,
                } = e.kind()
                {
                    debug!("No records for domain {}", domain);
                    return Ok(vec![]);
                }
                return Err(());
            }
            Ok(ips) => ips,
        };

        Ok(ips.iter().cloned().collect())
    }
}
