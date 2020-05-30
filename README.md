# Ebbflow Client

**NOTE** This is `Beta` quality as of now, and could use some more features and configurable settings!

This is the end-host client for [`ebbflow`](https://ebbflow.io). This is used to proxy SSH or TCP connections between ebbflow and your local server.

```
ebbflow --help
```

[![Build Status](https://dev.azure.com/ryan0769/ebbflow-test/_apis/build/status/ebbflow-io.ebbflow?branchName=azure-pipelines-testing)](https://dev.azure.com/ryan0769/ebbflow-test/_build/latest?definitionId=1&branchName=azure-pipelines-testing)

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
Coming Soon
```

RPM based Linux
```
Coming Soon
```

Windows
```
coming soon
```

More instructions coming soon.

## Getting Started

## Building & Testing

### Building Packages

**NOTE** To statically build packages, see [ebbflow-build](https://github.com/ebbflow-io/ebbflow-build).

- Install `cargo-deb`: https://crates.io/crates/cargo-deb
```
cargo deb
```

- Install `cargo-rpm`: https://crates.io/crates/cargo-rpm
```
cargo rpm build
```

- Install `cargo-wix`: https://crates.io/crates/cargo-wix
```
cargo wix
```

## Contributing

TODO

## Future Plans

TODO

## How It Works / Design

TODO
