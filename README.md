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

After following the [quickstart guide](https://preview.ebbflow.io/quickstart)

A quick way to test out ebbflow is to run the example code and point the client at that or to use the SSH feature.

For hosting and endpoint:
```
cargo run --example server &
cargo run -- tcp -k KEYFILE -p 8080 --dns YOURWEBSITE.COM &
curl YOURWEBSITE.COM
```

For SSHing:
```
cargo run -- ssh -k KEYFILE --accountid ACCTID &
ssh -J ACCTID@ebbflow.io USER@HOSTNAME
```

### Running the Client on Startup in the Background
If you log out of the terminal or your host reboots, `ebbflow` will not start back up and you won't be able to host your endpoint, or ssh to the host.

To fix this, and have it run in the background always, you can adapt the provided `.service` file with [systemd](https://wiki.archlinux.org/index.php/Systemd), which is present on all major linux distributions, including Raspbian.

NOTE That you could have multiple instances running at once, for example one for ssh and one for tcp!

1. Adapt the `ebbflow.service` file to your needs, the file has more instructions
1. Move the service file to the expected directory
    ```
    sudo cp ebbflow.service /etc/systemd/system/
    ```
1. Start up ebbflow
    ```
    sudo systemctl start ebbflow
    ```
1. Check that its up and running..
    ```
    sudo systemctl status ebbflow
    ```
1. Check that the hosting is actually running by hitting your endpoint/SSHing to your host
1. Once you've verified everything is good to go, enable this service to be started at bootup
    ```
    sudo systemctl enable ebbflow
    ```
1. Thats it! You can also stop the service using
    ```
    sudo systemctl stop ebbflow
    ```
    Or you can see the logs by doing
    ```
    sudo journalctl ebbflow
    ```

## Building & Testing

### Building Packages
- Install `cargo-deb`: https://crates.io/crates/cargo-deb
```
cargo deb
```

## Contributing

TODO

## Future Plans

TODO

## How It Works / Design

TODO
