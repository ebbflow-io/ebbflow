mod mockebb;

#[cfg(test)]
mod basic_tests_v0 {
    use crate::mockebb::listen_and_process;
    use tokio::net::TcpStream;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::prelude::*;
    use crate::mockebb::load_root;
    use ebbflow::{run_daemon, daemon::SharedInfo, config::EbbflowDaemonConfig, config::ConfigError, config::Endpoint};
    use futures::future::BoxFuture;
        use std::sync::Arc;

    const MOCKEBBSPAWNDELAY: Duration = Duration::from_millis(100);

    #[tokio::test]
    async fn basic_bytes() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Info)
            .init();
        let testclientport = 49193;
        let customerport = 49194;
        let serverport = 49195;

        tokio::spawn(listen_and_process(customerport, testclientport));
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;

        tokio::spawn(start_basic_daemon(testclientport, serverport));

        let serverconnhandle = tokio::spawn(get_one_proxied_connection(serverport));
        println!("Spawned");
        let mut customer = TcpStream::connect(format!("127.0.0.1:{}", customerport)).await.unwrap();
        println!("Connected");

        let mut server = serverconnhandle.await.unwrap().unwrap();

        // at this point, we have the customer conn and server conn, let's send some bytes.

        let writeme: [u8; 102] = [1; 102];
        customer.write_all(&writeme[..]).await.unwrap();

        let mut readme: [u8; 102] = [0; 102];
        server.read_exact(&mut readme[..]).await.unwrap();

        assert_eq!(readme[..], writeme[..]);
    }

    async fn get_one_proxied_connection(port: usize) -> Result<TcpStream, std::io::Error> {
        let mut listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await.unwrap();

        let (socket, _) = listener.accept().await?;
        Ok(socket)
    }

    async fn start_basic_daemon(ebbport: usize, serverport: usize) {

        let cfg = EbbflowDaemonConfig {
            key: "asdf".to_string(),
            endpoints: vec![
                Endpoint {
                    port: serverport as u16,
                    dns: "ebbflow.io".to_string(),
                    maxconns: 1000,
                    idleconns_override: None,
                    address_override: None,
                }
            ],
            enable_ssh: false,
            ssh: None,
        };
        let info = SharedInfo::new_with_ebbflow_overrides(format!("127.0.0.1:{}", ebbport).parse().unwrap(), "asdfasdf".to_string(), load_root()).await.unwrap();
        run_daemon(cfg, Arc::new(info), config_reload, dummyroot).await;
    }

    fn config_reload() -> BoxFuture<'static, Result<EbbflowDaemonConfig, ConfigError>> {
        Box::pin(async {
            Err(ConfigError::FileNotFound)
        })
    }

    fn dummyroot() -> Option<rustls::RootCertStore> {
        None
    }
}