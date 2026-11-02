# Changelog

All notable changes to vigil-rs are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [3.2.2] - 2026-11-02

### Fixed

- **vigild**: The reaper no longer loses a service's exit status ([`47d3163`](https://github.com/git001/vigil-rs/commit/47d316379ed97d233ab4e2a671ad8ecee6d38586))

### Security

- **vigild**: Fully drop group privileges when switching user ([`2483705`](https://github.com/git001/vigil-rs/commit/2483705f6dbfe1f603b667dd23b2367831385403))
- Harden control socket creation and verify daemon in client ([`f6f905a`](https://github.com/git001/vigil-rs/commit/f6f905a515e77d282866bf1074bdb1b5edb512ec))

## [3.2.1] - 2026-05-08

No user-facing changes.

## [3.2.0] - 2026-05-08

### Added

- **vigil-log-relay**: Multi-container streaming via --container all|n1,n2 ([`477f5da`](https://github.com/git001/vigil-rs/commit/477f5dae3d7c4d85d0edabecd660574f4e7b6a8c))

### Fixed

- **vigild**: Collapse match guard to satisfy clippy::collapsible_match (Rust 1.95) ([`0bd3006`](https://github.com/git001/vigil-rs/commit/0bd3006e5f574866ec6675883cf0c29aefe31ccf))

## [3.1.2] - 2026-03-30

### Added

- **vigil-log-relay**: Support RUST_LOG env var for log level ([`9c84f3d`](https://github.com/git001/vigil-rs/commit/9c84f3d3bb9642d23cc91c7633785aa54536a284))

## [3.1.1] - 2026-03-23

### Added

- **vigil-log-relay**: Add --stdout-sink flag ([`8a38a37`](https://github.com/git001/vigil-rs/commit/8a38a377b8a9ec7d34ae837db766eaaa58f40de9))

## [3.1.0] - 2026-03-23

### Added

- **vigild**: Debug logging for checks and replan change detection ([`7b79f46`](https://github.com/git001/vigil-rs/commit/7b79f462016f55fa307627dc58c66f322b7e33ca))
- **vigild**: Http check no-follow-redirects + insecure-proxy flags ([`a715ace`](https://github.com/git001/vigil-rs/commit/a715ace7ce7b5a27eb029fbc746d326a537ce909))

## [3.0.3] - 2026-03-23

### Added

- **vigild**: Add insecure-proxy flag to HTTP check ([`cc02861`](https://github.com/git001/vigil-rs/commit/cc0286155479f20fba09c54724b57ce5ad7adfb6))

## [3.0.2] - 2026-03-23

### Added

- **vigild**: Configurable success-statuses for HTTP checks ([`ae65edb`](https://github.com/git001/vigil-rs/commit/ae65edb90487614a92214ebf39f08a95028a3e4b))

## [3.0.1] - 2026-03-23

### Security

- Cargo update aws-lc-rs 1.16.2 (security: RUSTSEC-2026-0044, -0048, -0049) ([`68657c7`](https://github.com/git001/vigil-rs/commit/68657c70013983c70caa213e11d05a4340a406ea))

## [3.0.0] - 2026-03-21

### Added

- **vigil-log-relay**: K8s stdout/stderr separation, tcp_sink fix, --no-stream-param ([`d6d0d59`](https://github.com/git001/vigil-rs/commit/d6d0d59d1504514d0e515a5cdf3696c9be4f763f))
- **vigild**: Alerts, mTLS, HTTP check TLS, before/requires ordering, cascade fix ([`2ffd7c7`](https://github.com/git001/vigil-rs/commit/2ffd7c7ef0970170770525e180607a49d5f7609d))
- **vigil+vigil-types**: Client refactor, no_proxy, plan types, CLI improvements ([`06d9cec`](https://github.com/git001/vigil-rs/commit/06d9cec5c3cbb4113343a33bcbe0cf32179fe3c2))

## [2.0.4] - 2026-03-20

### Added

- Add version flag to vigil-log-relay ([`384052e`](https://github.com/git001/vigil-rs/commit/384052eabdee2243666c346290c6b72d35b5e68d))

## [2.0.3] - 2026-03-20

### Changed

- Use dedicated USER 1001 and remove cli paramters from vigild ([`139f6b0`](https://github.com/git001/vigil-rs/commit/139f6b0ce9a82affca3009105af5b56e64ed71da))

## [2.0.2] - 2026-03-20

### Changed

- Add vigil-log-relay, bash and curl to Dockerfile ([`9ef0c93`](https://github.com/git001/vigil-rs/commit/9ef0c935936f5d914ef9210901b1475cff7703bb))

## [2.0.1] - 2026-03-19

### Added

- Add json logformat to vigil-log-relay ([`04342f8`](https://github.com/git001/vigil-rs/commit/04342f81f5ca260426263f6d5358012cb6dab2fb))

## [2.0.0] - 2026-03-19

### Added

- Before/requires ordering, HTTP check TLS options, cascade fix ([`f98bf87`](https://github.com/git001/vigil-rs/commit/f98bf87a31d875c2dbed201953f7a2750d86d2ad))
- Replace vigil-http-streamer with vigil-log-relay ([`fe4994b`](https://github.com/git001/vigil-rs/commit/fe4994b0cd948b533cbeb0b770b0bfe6cdcc7dc4))

## [1.0.0] - 2026-03-19

### Added

- Initial release of vigil-rs v1.0.0 ([`892d9e6`](https://github.com/git001/vigil-rs/commit/892d9e6a759e94e0d3eec17a8ed33d74b4c3a999))

[3.2.2]: https://github.com/git001/vigil-rs/compare/v3.2.1...v3.2.2
[3.2.1]: https://github.com/git001/vigil-rs/compare/v3.2.0...v3.2.1
[3.2.0]: https://github.com/git001/vigil-rs/compare/v3.1.2...v3.2.0
[3.1.2]: https://github.com/git001/vigil-rs/compare/v3.1.1...v3.1.2
[3.1.1]: https://github.com/git001/vigil-rs/compare/v3.1.0...v3.1.1
[3.1.0]: https://github.com/git001/vigil-rs/compare/v3.0.3...v3.1.0
[3.0.3]: https://github.com/git001/vigil-rs/compare/v3.0.2...v3.0.3
[3.0.2]: https://github.com/git001/vigil-rs/compare/v3.0.1...v3.0.2
[3.0.1]: https://github.com/git001/vigil-rs/compare/v3.0.0...v3.0.1
[3.0.0]: https://github.com/git001/vigil-rs/compare/v2.0.4...v3.0.0
[2.0.4]: https://github.com/git001/vigil-rs/compare/v2.0.3...v2.0.4
[2.0.3]: https://github.com/git001/vigil-rs/compare/v2.0.2...v2.0.3
[2.0.2]: https://github.com/git001/vigil-rs/compare/v2.0.1...v2.0.2
[2.0.1]: https://github.com/git001/vigil-rs/compare/v2.0.0...v2.0.1
[2.0.0]: https://github.com/git001/vigil-rs/compare/v1.0.0...v2.0.0
[1.0.0]: https://github.com/git001/vigil-rs/releases/tag/v1.0.0

