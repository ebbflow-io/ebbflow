mod mockebb;
#[macro_use]
extern crate log;

#[cfg(test)]
mod basic_tests_v0 {
    use crate::mockebb::listen_and_process;
    use crate::mockebb::load_root;
    use ebbflow::{
        config::ConfigError, config::EbbflowDaemonConfig, config::Endpoint, daemon::SharedInfo,
        run_daemon,
    };
    use futures::future::BoxFuture;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::net::TcpStream;
    use tokio::prelude::*;
    use tokio::sync::watch::{channel, Sender, Receiver};
    use tokio::sync::Notify;

    const MOCKEBBSPAWNDELAY: Duration = Duration::from_millis(100);

    #[tokio::test]
    async fn basic_bytes() {
        // logger();
        let testclientport = 49193;
        let customerport = 49194;
        let serverport = 49195;

        tokio::spawn(listen_and_process(customerport, testclientport));
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned ebb");

        start_basic_daemon(testclientport, serverport).await;
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned daemon");

        let serverconnhandle = tokio::spawn(get_one_proxied_connection(serverport));
        info!("Spawned server");
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        let mut customer = TcpStream::connect(format!("127.0.0.1:{}", customerport))
            .await
            .unwrap();
        info!("Connected");

        let mut server = serverconnhandle.await.unwrap().unwrap();

        // at this point, we have the customer conn and server conn, let's send some bytes.

        let writeme: [u8; 102] = [1; 102];
        customer.write_all(&writeme[..]).await.unwrap();
        info!("Wrote Customer Stuff");

        let mut readme: [u8; 102] = [0; 102];
        server.read_exact(&mut readme[..]).await.unwrap();
        info!("Read Server Stuff");

        assert_eq!(readme[..], writeme[..]);
    }

    fn logger() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Debug)
            .init();
    }

    #[tokio::test]
    async fn two_connections() {
        //logger();
        let testclientport = 49196;
        let customerport = 49197;
        let serverport = 49198;

        tokio::spawn(listen_and_process(customerport, testclientport));
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned ebb");

        start_basic_daemon(testclientport, serverport).await;
        info!("Spawned daemon");

        let serverconnhandle = tokio::spawn(get_one_proxied_connection(serverport));
        info!("Spawned server");
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        let mut customer = TcpStream::connect(format!("127.0.0.1:{}", customerport))
            .await
            .unwrap();
        info!("Connected");

        let mut server = serverconnhandle.await.unwrap().unwrap();

        // at this point, we have the customer conn and server conn, let's send some bytes.
        let writeme: [u8; 102] = [1; 102];
        customer.write_all(&writeme[..]).await.unwrap();
        info!("Wrote Customer Stuff");

        let mut readme: [u8; 102] = [0; 102];
        server.read_exact(&mut readme[..]).await.unwrap();
        info!("Read Server Stuff");

        assert_eq!(readme[..], writeme[..]);

        let serverconnhandle2 = tokio::spawn(get_one_proxied_connection(serverport));
        info!("spawned second server");
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        let mut customer2 = TcpStream::connect(format!("127.0.0.1:{}", customerport))
            .await
            .unwrap();
        info!("Connected 2");
        let mut server2 = serverconnhandle2.await.unwrap().unwrap();
        info!("both now ready");

        let writeme: [u8; 102] = [1; 102];
        customer2.write_all(&writeme[..]).await.unwrap();
        info!("Wrote Customer2 Stuff");

        let mut readme: [u8; 102] = [0; 102];
        server2.read_exact(&mut readme[..]).await.unwrap();
        info!("Read Server2 Stuff");
        assert_eq!(readme[..], writeme[..]);

        // Test the other stuff is still going.
        let writeme: [u8; 102] = [1; 102];
        customer.write_all(&writeme[..]).await.unwrap();
        info!("Wrote Customer Stuff again");

        let mut readme: [u8; 102] = [0; 102];
        server.read_exact(&mut readme[..]).await.unwrap();
        info!("Read Server Stuff again");

        assert_eq!(readme[..], writeme[..]);

    }

    async fn get_one_proxied_connection(port: usize) -> Result<TcpStream, std::io::Error> {
        let mut listener = TcpListener::bind(format!("127.0.0.1:{}", port))
            .await
            .unwrap();

        let (socket, _) = listener.accept().await?;
        info!("Got a proxied connection to the server, giving back");
        Ok(socket)
    }

    async fn start_basic_daemon(ebbport: usize, serverport: usize) {
        let info = SharedInfo::new_with_ebbflow_overrides(
            format!("127.0.0.1:{}", ebbport).parse().unwrap(),
            load_root(),
            "hostname".to_string(),
        )
        .await
        .unwrap();

        // info.update_key("asdfasdf".to_string());
        
        tokio::spawn(async move {
            run_daemon( Arc::new(info), config_reload_maker(serverport), dummyroot, Arc::new(Notify::new())).await;
        });
    }

    fn config_reload_maker(serverport: usize) -> std::pin::Pin<Box<dyn Fn() -> BoxFuture<'static, Result<EbbflowDaemonConfig, ConfigError>>  + Send + Sync + 'static>> {
        let z = serverport as u16;
        Box::pin(move || {
            Box::pin(async move { 
                let cfg = EbbflowDaemonConfig {
                    key: "asdf".to_string(),
                    endpoints: vec![Endpoint {
                        port: z,
                        dns: "ebbflow.io".to_string(),
                        maxconns: 1000,
                        idleconns_override: Some(1),
                        address_override: None,
                        enabled: true,
                    }],
                    enable_ssh: false,
                    ssh: None,
                };
                Ok(cfg)
            })
        })
    }

    fn dummyroot() -> Option<rustls::RootCertStore> {
        None
    }
}
