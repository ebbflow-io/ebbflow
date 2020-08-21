#[macro_use]
extern crate log;
extern crate env_logger;

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Error, Response, Server};
use log::LevelFilter;

#[tokio::main]
async fn main() {
    env_logger::builder().filter_level(LevelFilter::Info).init();

    // Construct our SocketAddr to listen on...
    let addr = ([127, 0, 0, 1], 8080).into();

    // And a MakeService to  handle each connection...
    let make_service = make_service_fn(|_| async {
        Ok::<_, Error>(service_fn(|_req| async {
            info!("Received request, returning response");
            Ok::<_, Error>(Response::new(Body::from(format!(
                "Hello, World! Hostname: {:?}\n",
                hostname::get().unwrap()
            ))))
        }))
    });

    // Then bind and serve...
    let server = Server::bind(&addr).serve(make_service);

    // Finally, spawn `server` onto an Executor...
    info!("Spawning server on 8080..");
    if let Err(e) = server.await {
        error!("server error: {}", e);
    }
}
