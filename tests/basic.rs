mod mockebb;
#[macro_use]
extern crate log;

#[cfg(test)]
mod basic_tests_v0 {
    use crate::mockebb::listen_and_process;
    use crate::mockebb::load_root;
    use ebbflow::{
        config::{ConfigError, EbbflowDaemonConfig, Endpoint, HealthCheck, HealthCheckType, Ssh},
        daemon::{HealthOverall, SharedInfo},
        run_daemon, DaemonEndpointStatus, DaemonRunner, DaemonStatusMeta,
    };
    use futures::future::BoxFuture;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::net::TcpStream;
    use tokio::prelude::*;
    use tokio::sync::Mutex;
    use tokio::{sync::Notify, time::delay_for};

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

        let (_notify, _arcmutex, _) =
            start_basic_daemon(testclientport, ezconfigendpoitnonly(serverport as u16)).await;
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

    #[allow(unused)]
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

        let (_notify, _arcmutex, _) =
            start_basic_daemon(testclientport, ezconfigendpoitnonly(serverport as u16)).await;
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

    #[tokio::test]
    async fn basic_bytes_ssh() {
        // logger();
        let testclientport = 49183;
        let customerport = 49184;
        let serverport = 49185;

        tokio::spawn(listen_and_process(customerport, testclientport));
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned ebb");

        let x = "testhostname.isawaseoma-fasdf.adf1".to_string();

        let cfg = EbbflowDaemonConfig {
            endpoints: vec![],
            ssh: Some(Ssh {
                port: serverport, // We need to override the SSH port or else it will hit the actual ssh server the host
                hostname_override: Some(x),
                enabled: true,
                maxconns: 100,
                maxidle: 2,
            }),
            loglevel: None,
        };

        let (_notify, _arcmutex, _) = start_basic_daemon(testclientport, cfg).await;
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned daemon");

        let serverconnhandle = tokio::spawn(get_one_proxied_connection(serverport as usize));
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

    #[tokio::test]
    async fn endpoint_start_disabled_fails_then_enable_works() {
        // logger();
        let testclientport = 49543;
        let customerport = 49544;
        let serverport = 49545;

        tokio::spawn(listen_and_process(customerport, testclientport));
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned ebb");

        let mut initial_endpoint = ezconfigendpoitnonly(serverport as u16);
        initial_endpoint.endpoints.get_mut(0).unwrap().enabled = false;

        let (notify, arcmutex, _) = start_basic_daemon(testclientport, initial_endpoint).await;
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned daemon");

        let serverconnhandle = tokio::spawn(get_one_proxied_connection(serverport));
        info!("Spawned server early that should return successfully later");
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;

        // We should not be able to connect
        let should_err = {
            match TcpStream::connect(format!("127.0.0.1:{}", customerport)).await {
                Ok(mut s) => {
                    info!("Connected Client");
                    tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
                    // We should be disconnected soon, or not be able to write
                    let _r = s.write(&[0; 4][..]).await;
                    info!("Wrote Customer Stuff");
                    let mut buf = vec![0; 10];
                    let r = s.read(&mut buf[..]).await;
                    info!("Read Customer Stuff {:?}", r);
                    if let Ok(0) = r {
                        Err(std::io::Error::from(std::io::ErrorKind::NotConnected))
                    } else {
                        r
                    }
                }
                Err(e) => Err(e),
            }
        };
        assert!(should_err.is_err());

        let mut cfg = ezconfigendpoitnonly(serverport as u16);
        cfg.endpoints.get_mut(0).unwrap().enabled = true;
        {
            let mut x = arcmutex.lock().await;
            *x = cfg;
        }
        notify.notify();
        tokio::time::delay_for(MOCKEBBSPAWNDELAY * 2).await;

        // at this point, we have the customer conn and server conn, let's send some bytes.
        let mut customer = TcpStream::connect(format!("127.0.0.1:{}", customerport))
            .await
            .unwrap();
        info!("Connected Client that should succeed");
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        let mut server = serverconnhandle.await.unwrap().unwrap(); // we spawned ONE Accept thing earlier, it should have NOT resolved and only NOW resolves once we connect again
        let writeme: [u8; 10212] = [5; 10212];
        customer.write_all(&writeme[..]).await.unwrap();
        info!("Wrote Customer Stuff should succeed");
        let mut readme: [u8; 10212] = [0; 10212];
        server.read_exact(&mut readme[..]).await.unwrap();
        info!("Read Server Stuff should succeed");
        assert_eq!(readme[..], writeme[..]);
    }

    #[tokio::test]
    async fn endpoint_start_enabled_disable_reenable() {
        // logger();

        let testclientport = 49143;
        let customerport = 49144;
        let serverport = 49145;

        tokio::spawn(listen_and_process(customerport, testclientport));
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned ebb");

        let (notify, arcmutex, _) =
            start_basic_daemon(testclientport, ezconfigendpoitnonly(serverport as u16)).await;
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned daemon");

        let serverconnhandle = tokio::spawn(get_one_proxied_connection(serverport));
        info!("Spawned server");
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        let mut customer = TcpStream::connect(format!("127.0.0.1:{}", customerport))
            .await
            .unwrap();
        info!("Connected I");

        let mut server = serverconnhandle.await.unwrap().unwrap();

        // at this point, we have the customer conn and server conn, let's send some bytes.
        let writeme: [u8; 102] = [1; 102];
        customer.write_all(&writeme[..]).await.unwrap();
        info!("Wrote Customer Stuff I");

        let mut readme: [u8; 102] = [0; 102];
        server.read_exact(&mut readme[..]).await.unwrap();
        info!("Read Server Stuff I");

        assert_eq!(readme[..], writeme[..]);
        let serverconnhandle = tokio::spawn(get_one_proxied_connection(serverport));

        // Now we shut it off and assume we cannot connect
        let mut cfg = ezconfigendpoitnonly(serverport as u16);
        cfg.endpoints.get_mut(0).unwrap().enabled = false;
        {
            let mut x = arcmutex.lock().await;
            *x = cfg;
        }
        notify.notify();
        tokio::time::delay_for(MOCKEBBSPAWNDELAY * 2).await;

        // we have something ready
        // We should not be able to connect
        let should_err = {
            match TcpStream::connect(format!("127.0.0.1:{}", customerport)).await {
                Ok(mut s) => {
                    info!("Connected Client II");
                    tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
                    // We should be disconnected soon, or not be able to write
                    let _r = s.write(&[0; 4][..]).await;
                    info!("Wrote Customer Stuff II");
                    let mut buf = vec![0; 10];
                    let r = s.read(&mut buf[..]).await;
                    info!("Read Customer Stuff II {:?}", r);
                    if let Ok(0) = r {
                        Err(std::io::Error::from(std::io::ErrorKind::NotConnected))
                    } else {
                        r
                    }
                }
                Err(e) => Err(e),
            }
        };
        assert!(should_err.is_err());

        let mut cfg = ezconfigendpoitnonly(serverport as u16);
        cfg.endpoints.get_mut(0).unwrap().enabled = true;
        {
            let mut x = arcmutex.lock().await;
            *x = cfg;
        }
        notify.notify();
        tokio::time::delay_for(MOCKEBBSPAWNDELAY * 2).await;

        // at this point, we have the customer conn and server conn, let's send some bytes.
        let mut customer = TcpStream::connect(format!("127.0.0.1:{}", customerport))
            .await
            .unwrap();
        info!("Connected Client III");
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        let mut server = serverconnhandle.await.unwrap().unwrap(); // we spawned ONE Accept thing earlier, it should have NOT resolved and only NOW resolves once we connect again
        let writeme: [u8; 10212] = [5; 10212];
        customer.write_all(&writeme[..]).await.unwrap();
        info!("Wrote Customer Stuff III");
        let mut readme: [u8; 10212] = [0; 10212];
        server.read_exact(&mut readme[..]).await.unwrap();
        info!("Read Server Stuff III");
        assert_eq!(readme[..], writeme[..]);
    }

    #[tokio::test]
    async fn ssh_start_enabled_disable_reenable() {
        // logger();
        let testclientport = 43143;
        let customerport = 43144;
        let serverport = 43145;

        tokio::spawn(listen_and_process(customerport, testclientport));
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned ebb");

        let hostname = "asdf31".to_string();
        let mut cfg = EbbflowDaemonConfig {
            endpoints: vec![],
            ssh: Some(Ssh {
                port: serverport, // We need to override the SSH port or else it will hit the actual ssh server the host
                hostname_override: Some(hostname),
                enabled: true,
                maxconns: 100,
                maxidle: 2,
            }),
            loglevel: None,
        };

        let (notify, arcmutex, _) = start_basic_daemon(testclientport, cfg.clone()).await;
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned daemon");

        let serverconnhandle = tokio::spawn(get_one_proxied_connection(serverport as usize));
        info!("Spawned server");
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        let mut customer = TcpStream::connect(format!("127.0.0.1:{}", customerport))
            .await
            .unwrap();
        info!("Connected I");

        let mut server = serverconnhandle.await.unwrap().unwrap();

        // at this point, we have the customer conn and server conn, let's send some bytes.
        let writeme: [u8; 102] = [1; 102];
        customer.write_all(&writeme[..]).await.unwrap();
        info!("Wrote Customer Stuff I");

        let mut readme: [u8; 102] = [0; 102];
        server.read_exact(&mut readme[..]).await.unwrap();
        info!("Read Server Stuff I");

        assert_eq!(readme[..], writeme[..]);
        let serverconnhandle = tokio::spawn(get_one_proxied_connection(serverport as usize));

        // Now we shut it off and assume we cannot connect
        cfg.ssh.as_mut().unwrap().enabled = false;
        {
            let mut x = arcmutex.lock().await;
            *x = cfg.clone();
        }
        notify.notify();
        tokio::time::delay_for(MOCKEBBSPAWNDELAY * 2).await;

        // we have something ready
        // We should not be able to connect
        let should_err = {
            match TcpStream::connect(format!("127.0.0.1:{}", customerport)).await {
                Ok(mut s) => {
                    info!("Connected Client II");
                    tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
                    // We should be disconnected soon, or not be able to write
                    let _r = s.write(&[0; 4][..]).await;
                    info!("Wrote Customer Stuff II");
                    let mut buf = vec![0; 10];
                    let r = s.read(&mut buf[..]).await;
                    info!("Read Customer Stuff II {:?}", r);
                    if let Ok(0) = r {
                        Err(std::io::Error::from(std::io::ErrorKind::NotConnected))
                    } else {
                        r
                    }
                }
                Err(e) => Err(e),
            }
        };
        assert!(should_err.is_err());

        cfg.ssh.as_mut().unwrap().enabled = true;
        {
            let mut x = arcmutex.lock().await;
            *x = cfg;
        }
        notify.notify();
        tokio::time::delay_for(MOCKEBBSPAWNDELAY * 2).await;

        // at this point, we have the customer conn and server conn, let's send some bytes.
        let mut customer = TcpStream::connect(format!("127.0.0.1:{}", customerport))
            .await
            .unwrap();
        info!("Connected Client III");
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        let mut server = serverconnhandle.await.unwrap().unwrap(); // we spawned ONE Accept thing earlier, it should have NOT resolved and only NOW resolves once we connect again
        let writeme: [u8; 10212] = [5; 10212];
        customer.write_all(&writeme[..]).await.unwrap();
        info!("Wrote Customer Stuff III");
        let mut readme: [u8; 10212] = [0; 10212];
        server.read_exact(&mut readme[..]).await.unwrap();
        info!("Read Server Stuff III");
        assert_eq!(readme[..], writeme[..]);
    }

    #[tokio::test]
    async fn ssh_start_disabled_fails_then_enable_works() {
        // logger();
        let testclientport = 49443;
        let customerport = 49444;
        let serverport = 49445;

        tokio::spawn(listen_and_process(customerport, testclientport));
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned ebb");

        let hostname = "asdf31".to_string();
        let mut cfg = EbbflowDaemonConfig {
            endpoints: vec![],
            ssh: Some(Ssh {
                port: serverport, // We need to override the SSH port or else it will hit the actual ssh server the host
                hostname_override: Some(hostname),
                enabled: false,
                maxconns: 100,
                maxidle: 2,
            }),
            loglevel: None,
        };

        let (notify, arcmutex, _) = start_basic_daemon(testclientport, cfg.clone()).await;
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned daemon");

        let serverconnhandle = tokio::spawn(get_one_proxied_connection(serverport as usize));
        info!("Spawned server early that should return successfully later");
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;

        // We should not be able to connect
        let should_err = {
            match TcpStream::connect(format!("127.0.0.1:{}", customerport)).await {
                Ok(mut s) => {
                    info!("Connected Client");
                    tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
                    // We should be disconnected soon, or not be able to write
                    let _r = s.write(&[0; 4][..]).await;
                    info!("Wrote Customer Stuff");
                    let mut buf = vec![0; 10];
                    let r = s.read(&mut buf[..]).await;
                    info!("Read Customer Stuff {:?}", r);
                    if let Ok(0) = r {
                        Err(std::io::Error::from(std::io::ErrorKind::NotConnected))
                    } else {
                        r
                    }
                }
                Err(e) => Err(e),
            }
        };
        assert!(should_err.is_err());

        cfg.ssh.as_mut().unwrap().enabled = true;
        {
            let mut x = arcmutex.lock().await;
            *x = cfg;
        }
        notify.notify();
        tokio::time::delay_for(MOCKEBBSPAWNDELAY * 2).await;

        // at this point, we have the customer conn and server conn, let's send some bytes.
        let mut customer = TcpStream::connect(format!("127.0.0.1:{}", customerport))
            .await
            .unwrap();
        info!("Connected Client that should succeed");
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        let mut server = serverconnhandle.await.unwrap().unwrap(); // we spawned ONE Accept thing earlier, it should have NOT resolved and only NOW resolves once we connect again
        let writeme: [u8; 10212] = [5; 10212];
        customer.write_all(&writeme[..]).await.unwrap();
        info!("Wrote Customer Stuff should succeed");
        let mut readme: [u8; 10212] = [0; 10212];
        server.read_exact(&mut readme[..]).await.unwrap();
        info!("Read Server Stuff should succeed");
        assert_eq!(readme[..], writeme[..]);
    }

    #[tokio::test]
    async fn just_status_check() {
        let testclientport = 49155;
        let customerport = 49156;

        let e0 = "mysite.com".to_string();
        let ep0 = 13413;
        let e1 = "othersite.com".to_string();
        let ep1 = 12341;
        let hn = "hostname".to_string();
        let sshp = 131;

        let cfg = EbbflowDaemonConfig {
            endpoints: vec![
                Endpoint {
                    port: ep0,
                    dns: e0.clone(),
                    maxconns: 1000,
                    maxidle: 2,
                    enabled: true,
                    healthcheck: None,
                },
                Endpoint {
                    port: ep1,
                    dns: e1.clone(),
                    maxconns: 1000,
                    maxidle: 3,
                    enabled: true,
                    healthcheck: None,
                },
            ],
            ssh: Some(Ssh {
                maxconns: 1,
                port: sshp,
                enabled: true,
                maxidle: 1,
                hostname_override: Some(hn.clone()),
            }),
            loglevel: None,
        };

        tokio::spawn(listen_and_process(customerport, testclientport));
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned ebb");

        let (_notify, _arcmutex, daemon) = start_basic_daemon(testclientport, cfg).await;
        tokio::time::delay_for(MOCKEBBSPAWNDELAY * 3).await;
        info!("Spawned daemon");

        let status = daemon.status().await;

        assert_eq!(DaemonStatusMeta::Good, status.meta);
        assert_eq!(
            Some((
                hn,
                DaemonEndpointStatus::Enabled {
                    active: 0,
                    idle: 1,
                    health: Some(HealthOverall::NOT_CONFIGURED),
                }
            )),
            status.ssh
        );

        assert_eq!(2, status.endpoints.len());

        for (endpoint, status) in status.endpoints.iter() {
            println!("e {} s {:?}", endpoint, status);
            if e0 == endpoint.as_str() {
                assert_eq!(
                    &DaemonEndpointStatus::Enabled {
                        active: 0,
                        idle: 2,
                        health: Some(HealthOverall::NOT_CONFIGURED),
                    },
                    status
                );
            } else if e1 == endpoint.as_str() {
                assert_eq!(
                    &DaemonEndpointStatus::Enabled {
                        active: 0,
                        idle: 3,
                        health: Some(HealthOverall::NOT_CONFIGURED),
                    },
                    status
                );
            } else {
                panic!("unexpected")
            }
        }
    }

    #[tokio::test]
    async fn health_check_starts_up_single_connection_different_port() {
        // logger();
        let testclientport = 35001;
        let customerport = 35002;
        let serverport: usize = 35003;
        let healthcheckport: usize = 35004;

        tokio::spawn(listen_and_process(customerport, testclientport));
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned ebb");

        let (_notify, _arcmutex, _) = start_basic_daemon(
            testclientport,
            EbbflowDaemonConfig {
                endpoints: vec![Endpoint {
                    port: serverport as u16,
                    dns: "ebbflow.io".to_string(),
                    maxconns: 1000,
                    maxidle: 1,
                    enabled: true,
                    healthcheck: Some(HealthCheck {
                        port: Some(healthcheckport as u16),
                        consider_healthy_threshold: None,
                        consider_unhealthy_threshold: None,
                        r#type: HealthCheckType::TCP,
                        frequency_secs: Some(2),
                    }),
                }],
                ssh: None,
                loglevel: None,
            },
        )
        .await;
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        info!("Spawned daemon");

        // the health check server is a diff port to make it easy
        tokio::spawn(async move {
            spawn_healthy_tcplistener(healthcheckport).await.unwrap();
        });

        let serverconnhandle = tokio::spawn(get_one_proxied_connection(serverport));
        info!("Spawned server");

        tokio::time::delay_for(Duration::from_secs(3)).await;

        // Here we should fail for a few seconds (6)
        // We should not be able to connect
        let should_err = {
            match TcpStream::connect(format!("127.0.0.1:{}", customerport)).await {
                Ok(mut s) => {
                    info!("Connected Client");
                    tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
                    // We should be disconnected soon, or not be able to write
                    let _r = s.write(&[0; 4][..]).await;
                    info!("Wrote Customer Stuff");
                    let mut buf = vec![0; 10];
                    let r = s.read(&mut buf[..]).await;
                    info!("Read Customer Stuff {:?}", r);
                    if let Ok(0) = r {
                        Err(std::io::Error::from(std::io::ErrorKind::NotConnected))
                    } else {
                        r
                    }
                }
                Err(e) => Err(e),
            }
        };
        assert!(should_err.is_err());

        // Now we should be good!
        tokio::time::delay_for(Duration::from_secs(7)).await;

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

    async fn spawn_healthy_tcplistener(port: usize) -> Result<TcpStream, std::io::Error> {
        let mut listener = TcpListener::bind(format!("127.0.0.1:{}", port))
            .await
            .unwrap();

        loop {
            let (socket, _) = listener.accept().await?;
            info!("Got a connection, shoving it to another task for a second then killing it");
            tokio::spawn(async move {
                delay_for(Duration::from_secs(1)).await;
                drop(socket);
            });
        }
    }

    async fn get_one_proxied_connection(port: usize) -> Result<TcpStream, std::io::Error> {
        let mut listener = TcpListener::bind(format!("127.0.0.1:{}", port))
            .await
            .unwrap();

        let (socket, _) = listener.accept().await?;
        info!("Got a proxied connection to the server, giving back");
        Ok(socket)
    }

    async fn start_basic_daemon(
        ebbport: usize,
        cfg: EbbflowDaemonConfig,
    ) -> (
        Arc<Notify>,
        Arc<Mutex<EbbflowDaemonConfig>>,
        Arc<DaemonRunner>,
    ) {
        let info = SharedInfo::new_with_ebbflow_overrides(
            format!("127.0.0.1:{}", ebbport).parse().unwrap(),
            "preview.ebbflow.io".to_string(),
            load_root(),
        )
        .await
        .unwrap();

        let (am, f) = config_with_reloading(cfg);

        let nc = Arc::new(Notify::new());
        let n = nc.clone();

        let m = run_daemon(Arc::new(info), f, n.clone()).await;

        (nc, am, m)
    }

    fn ezconfigendpoitnonly(port: u16) -> EbbflowDaemonConfig {
        EbbflowDaemonConfig {
            endpoints: vec![Endpoint {
                port,
                dns: "ebbflow.io".to_string(),
                maxconns: 1000,
                maxidle: 1,
                enabled: true,
                healthcheck: None,
            }],
            ssh: None,
            loglevel: None,
        }
    }

    fn config_with_reloading(
        cfg: EbbflowDaemonConfig,
    ) -> (
        Arc<Mutex<EbbflowDaemonConfig>>,
        std::pin::Pin<
            Box<
                dyn Fn() -> BoxFuture<
                        'static,
                        Result<(Option<EbbflowDaemonConfig>, Option<String>), ConfigError>,
                    > + Send
                    + Sync
                    + 'static,
            >,
        >,
    ) {
        let cfg: Arc<Mutex<EbbflowDaemonConfig>> = Arc::new(Mutex::new(cfg));

        let c = cfg.clone();
        (
            cfg,
            Box::pin(move || {
                let cc = c.clone();
                Box::pin(
                    async move { Ok((Some(cc.lock().await.clone()), Some("asdf".to_string()))) },
                )
            }),
        )
    }
}
