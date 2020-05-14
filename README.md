# Ebbflow Client

**NOTE** This is `Beta` quality as of now, and could use some more features and configurable settings!

This is the end-host client for [`ebbflow`](https://ebbflow.io). This is used to proxy SSH or TCP connections between ebbflow and your local server.

```
ebbflow --help
```

The client has two modes, `tcp` and `ssh`, and you select one of these modes when starting the client. Check out the help pages for each command by doing `ebbflow tcp --help` or `ebbflow ssh --help`.

**NOTE** By default, only `1` connection is proxied. This is intended to allow you to test without having tons of logs thrown at you.

## Downloading, Updating, and Removing

Mac
```
brew tap ebbflow-io/ebbflow
brew install ebbflow

# To update
brew upgrade ebbflow

# To remove
brew remove ebbflow
```

Debian Linux
```
# Go to github releases, find the latest release, and copy the link to the .deb file
wget http://github.com/.../ebbflow_VERSION_amd64.deb
sudo apt install ./ebbflow_VERSION_amd64.deb

# To Update
# Go to github releases, find the latest release.. download
wget http://github.com/.../ebbflow_VERSION_amd64.deb
sudo apt install ./ebbflow_VERSION_amd64.deb


# To remove
sudo dpkg -P ebbflow
```

More instructions coming soon.

## Getting Started

## Building & Testing

### Building Packages
- Install `cargo-deb`: https://crates.io/crates/cargo-deb
```
cargo deb
```

- Install `cargo-rpm`: https://crates.io/crates/cargo-rpm
```
cargo rpm build
```

## Contributing

TODO

## Future Plans

TODO

## How It Works / Design

TODO
