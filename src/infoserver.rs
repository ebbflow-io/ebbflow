use crate::{config::addr_file_full, DaemonRunner};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server};
use std::sync::Arc;
use std::{convert::Infallible, net::SocketAddr, time::Duration};

pub async fn run_info_server(runner: Arc<DaemonRunner>) {
    loop {
        debug!("Starting info server..");
        // Let the OS pick an address
        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        // And a MakeService to handle each connection...
        let runnerc = runner.clone();
        let make_service = make_service_fn(move |_conn| {
            let runnerc = runnerc.clone();
            async move {
                Ok::<_, Infallible>(service_fn(move |req| {
                    let runnerccc = runnerc.clone();
                    async move { handle(req, runnerccc).await }
                }))
            }
        });

        // Then bind and serve...
        let addr = match Server::try_bind(&addr) {
            Ok(r) => r,
            Err(e) => {
                error!("Error spawning info server {:?}", e);
                tokio::time::delay_for(Duration::from_millis(5_000)).await;
                continue;
            }
        };
        let server = addr.serve(make_service);
        error!("Server spawning on addr {}", server.local_addr());

        if let Err(e) = crate::config::write_addr(&server.local_addr().to_string()).await {
            error!(
                "Could not write the address of the daemon info server to file {} {:?}",
                addr_file_full(),
                e
            );
            tokio::time::delay_for(Duration::from_millis(5_000)).await;
            continue;
        }

        // And run forever...
        if let Err(e) = server.await {
            eprintln!("server error: {}", e);
        }
    }
}

async fn handle(
    req: Request<Body>,
    runner: Arc<DaemonRunner>,
) -> Result<Response<Body>, Infallible> {
    Ok(match req.uri().path() {
        "/unstable_status" => {
            let status = runner.status().await;
            match serde_json::to_string_pretty(&status) {
                Ok(serialized) => Response::new(Body::from(serialized)),
                Err(e) => {
                    warn!(
                        "Could not serialize the status from the runner, status {:?} e {:?}",
                        status, e
                    );
                    Response::builder()
                        .status(500)
                        .body(Body::from("Error getting status"))
                        .unwrap()
                }
            }
        }
        _ => Response::builder()
            .status(400)
            .body(Body::from("Invalid Request"))
            .unwrap(),
    })
}
