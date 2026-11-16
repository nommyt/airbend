# Contributing

Use Discussions for setup questions and ideas, and the issue forms for reproducible bugs or
specific feature requests. Report security issues using [SECURITY.md](SECURITY.md).

The workspace separates the pure control engine, hardware backends, IPC, privileged daemon,
and Rune desktop UI. See [README.md](README.md) and [FEATURES.md](FEATURES.md) for the architecture
and implemented behavior. Changes should include focused validation of the affected behavior.

Run `cargo test --workspace --locked` before opening a pull request. Linux builds need Rust,
pkg-config, Fontconfig, libxkbcommon, Wayland, and libudev development libraries. Use the simulated
daemon for development that does not require physical fans:

```sh
cargo run -p airbend-daemon -- --mock --socket /tmp/airbend.sock --config /tmp/airbend.toml
AIRBEND_SOCKET=/tmp/airbend.sock cargo run -p airbend-ui
```

Preserve temperature failsafes, driver limits, takeover journaling, and automatic-control
restoration when changing hardware behavior. Explain whether validation used physical hardware
or a simulation. Do not commit local machine configurations, credentials, or build outputs.

Keep pull requests focused, describe the resulting behavior and validation, and update relevant
documentation. Changes are reviewed by the code owner and merged with squash merges. Contributions
are licensed under the repository's MIT license.
