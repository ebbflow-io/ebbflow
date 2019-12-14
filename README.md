# `ebbflow-client`

**NOTE** This is `Beta` quality as of now, and could use some more features and configurable settings!

This is the end-host client for [`ebbflow`](https://ebbflow.io). This is used to serve your endpoint by proxying connections between ebbflow and your webserver. 

```
ebb-client -c CERTFILE -k KEYFILE -p PORT --dns YOURWEBSITE.COM
```

**NOTE** By default, only `1` connection is proxied. This is intended to allow you to test without having tons of logs thrown at you.

## Getting Started

A quick way to test out ebbflow is to run the example code, and point the client at that, e.g.

```
cargo run --example server &
cargo run -- -c CERTFILE -k KEYFILE -p 8080 --dns YOURWEBSITE.COM
```

This will host your website against the example server which runs on port `8080`.

## Building & Testing

TODO

## Contributing 

TODO

## Future Plans

TODO

## How It Works / Design

TODO