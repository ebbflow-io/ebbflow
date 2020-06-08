# Ebbflow Client
This is the end-host client for [ebbflow](https://ebbflow.io). This is used to proxy SSH or TCP connections between ebbflow and your local server. It typically runs as a daemon which is initiated during the install process, but can also be ran directly which suits containers.

Full documentation can be found on the Ebbflow website: [Client Documentation](https://ebbflow.io/documentation#Client).

```
ebbflow --help
```

![Continuous Integration](https://github.com/ebbflow-io/ebbflow/workflows/Continuous%20Integration/badge.svg)

## Downloading, Updating, and Removing

Please visit Ebbflow's documentation for up to date instructions on installing and managing the client.

## Building & Testing

The client is built, tested, and packaged using the github action workflow configured in `.github/workflows`. When a release is expected, the released artifacts are downloaded to Ebbflow package servers and hosted through https://pkg.ebbflow.io.

As of now, testing is largely manual. The client is tested on various OSs & architectures before being released and vended. In the future, much of this testing could be completed in additional github workflow actions, but that is TBD.

### Local building
To build the client locally, you can simply fork/clone/download the repo and run `cargo build`, then continuing to execute the binaries manually. To execute with elevated privelages on linux/macos, run `sudo ./target/debug/ebbflow` or `sudo ./target/debug/ebbflowd`.

To build the various packages, you need the following tools.
- Install `cargo-deb`: https://crates.io/crates/cargo-deb (Only works on debian based OSs, ubuntu, debian..)
  - Building: `cargo deb`

- Install `cargo-rpm`: https://crates.io/crates/cargo-rpm (Only works on rpm-based architectures, fedora, opensuse, ..)
  - `cargo rpm build`

- Install `cargo-wix`: https://crates.io/crates/cargo-wix (Only works on windows)
  - cargo wix

## Contributing

Contributions are welcome! Submit a pull request and we can go from there.

## License

See LICENSE