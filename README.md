# Ebbflow Client
This is the end-host client for [ebbflow](https://ebbflow.io). This is used to proxy SSH or TCP connections between ebbflow and your local server. It typically runs as a daemon which is initiated during the install process, but can also be ran directly which suits containers.

Full documentation can be found on the Ebbflow website: [Client Documentation](https://ebbflow.io/documentation#client).

```
ebbflow --help
```

![Continuous Integration](https://github.com/ebbflow-io/ebbflow/workflows/Continuous%20Integration/badge.svg) ![Docker Image Size (tag)](https://img.shields.io/docker/image-size/ebbflow/ebbflow-client-linux-amd64/latest)

## Downloading, Updating, and Removing

Please visit Ebbflow's [client documentation](https://ebbflow.io/documentation#client) for up to date instructions on installing and managing the client:

- Docs: https://ebbflow.io/documentation#client
- RaspberryPi Guide: https://ebbflow.io/guides/raspberrypi

## Building & Testing

The client is built, tested, and packaged using the github action workflow configured in `.github/workflows`. When a release is expected, the released artifacts are downloaded to Ebbflow package servers and hosted through https://pkg.ebbflow.io.

As of now, testing is largely manual. The client is tested on various OSs & architectures before being released and vended. In the future, much of this testing could be completed in additional github workflow actions, but that is TBD.

To build the client locally, you can simply fork/clone/download the repo and run `cargo build`, then continuing to execute the binaries manually. To execute with elevated privileges on linux/macos, run `sudo ./target/debug/ebbflow` or `sudo ./target/debug/ebbflowd`.

To build the various packages, you need the following tools.

### Linux
- Install `cargo-deb`: https://crates.io/crates/cargo-deb (Only works on debian based OSs, ubuntu, debian..)
  - Building: `cargo deb`
- Install `cargo-rpm`: https://crates.io/crates/cargo-rpm (Only works on rpm-based architectures, fedora, opensuse, ..)
  - `cargo rpm build`

### MacOS
- Zip `ebbflow` and `ebbflowd`, just those 2 files
- Homebrew is used to distribute packages, see https://github.com/ebbflow-io/homebrew-ebbflow/

### Windows
- To build `cargo-wix`: https://crates.io/crates/cargo-wix (Only works on windows)
  - `cargo wix`
- Chocolatey is used to distribute packages, see https://github.com/ebbflow-io/chocolatey-ebbflow
- Winget is also used to distribute packages, `winget search ebbflow`

### Other Debian or RPM based
- Feel free to contact ebbflow and we can work to get packages published for you!

### Building from source, *nix Based (Arch, Nix)
- The simplest method is to just grab the 'ebbflow' binary, and run with `run-blocking`. This has no file-based dependencies if you pass the host key to during the command like so: `EBB_KEY=... ebbflow run-blocking --dns example.com --port 8000`. This is fully supported.
- Alternatively, you can run the daemon, but it does have file/directory dependencies, and you need to configure that. **Note** that this is uncharted waters, and its possible that the following instructions can change;
  - You will need to run the `ebbflowd` binary as a background service, as in `systemd` or similar.
  - The client uses the `/etc/ebbflow/` dir to store 3 files
    - `host.key` the private host key, should be `600` permissions, should (ideally) not be transferred between hosts, and should NEVER be entered into source control (i'm looking at you, nix config)
    - `config.yaml` the configuration file for the daemon. This file is read from to determine how the daemon should run, what endpoints exist, etc. This file is written to when endpoints are disabled or enabled, the log level changes, and a few other commands, via `ebbflow config ..`. So it is possible to have this file static, but I wouldn't recommend it.
    - `.daemonaddr` is a file the daemon writes its ipv4 socket addr to for its HTTP server. Running `ebbflow status` reads this file, then makes an http request for the status. The daemon will work fine, proxy-wise, if this file cannot be written or read from, it will just slowly loop on attempts to write it.
  - Now, you can change the root directory of which ebbflow looks for these three files by setting the `EBB_CFG_DIR` env var. The daemon will look at this first, and fall back to `/etc/ebbflow`. So you can provide this to another directory and ebbflow will look for the three files in this directory instead.
    - **IMPORTANT** If you change the directory, all invocations of the `ebbflow` command should have this env var passed to it as well! `ebbflow` and `ebbflowd` communicate via these files, so if `ebbflowd` is looking in `/your/custom/dir/` and `ebbflow` is looking in `/etc/ebbflow/`, then there will be issues..
  - If you package up ebbflow for another distro like nix or arch or whatever, and you want Ebbflow to take over ownership of your work, then email us at `support at ebbflow.io`.

## Contributing

Contributions are welcome! Submit a pull request and we can go from there.

## License

See LICENSE
