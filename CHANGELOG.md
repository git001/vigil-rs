# Changelog

All notable changes to vigil-rs are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Upgrading from v3: see the [migration guide](docs/user-guide/migration-v3-to-v4.md).

## [4.0.1] - 2026-11-09

### Fixed

- Multiline timeout defaults to 1s ([`3a185e5`](https://github.com/git001/vigil-rs/commit/3a185e511711ca04ce386eb8cb65b4825fb3b5ba))

## [4.0.0] - 2026-11-09

### ⚠ Breaking Changes

These need attention when upgrading; each is listed again below in its
section, with its migration note.

- **vigild**: Require a group for a user-id without passwd entry ([`2c26d5a`](https://github.com/git001/vigil-rs/commit/2c26d5a58d41335b415fdb247dd4ff98ac7235a9))
- Add --path-prefix as the single runtime directory ([`c59e292`](https://github.com/git001/vigil-rs/commit/c59e292b5624cd5dfa69f1545d571075a9479111))
- **vigild**: Stop passing vigild's environment to services by default ([`8cbad53`](https://github.com/git001/vigil-rs/commit/8cbad5366df0405dac10955b543358296151e50e))
- **vigild**: No anonymous admin over TLS, make /v1/checks open ([`6adfe40`](https://github.com/git001/vigil-rs/commit/6adfe405b6dc9c512ec12ff0740e03be38a390cd))
- Propagate the service's exit code from vigild ([`80e3f6c`](https://github.com/git001/vigil-rs/commit/80e3f6c1e1beaf426015bf7858ab08dc82336814))
- Readiness via sd_notify or checks (ready: process|notify|check) ([`4fc0b64`](https://github.com/git001/vigil-rs/commit/4fc0b6497aed599909530d92cf0673bbeb391ce9))
- Oneshot services and cron schedules (type: oneshot, schedule) ([`b11a4a5`](https://github.com/git001/vigil-rs/commit/b11a4a5e55cc45b32922ee24bc1fc3262352656e))
- Templates and env-file for services (minijinja) ([`01888d7`](https://github.com/git001/vigil-rs/commit/01888d7aee38c9c30e9bb462f4cedeb4aeea3d01))
- **vigil-types**: Keep startup and level when a layer does not set them ([`a26e62e`](https://github.com/git001/vigil-rs/commit/a26e62e392c91977d369cb0a590b116b22f1f410))
- **vigild**: Bound check period to 1s..24h ([`510baad`](https://github.com/git001/vigil-rs/commit/510baade16e410efdcf8be558954af280abc0db8))
- **vigild**: Validate alert queue limits ([`4dd10b0`](https://github.com/git001/vigil-rs/commit/4dd10b041fb1b4cf37fc827cf84659f7e9a0843d))
- Read layer files strictly and honour override ([`950b782`](https://github.com/git001/vigil-rs/commit/950b7820ea8959c4a1c0a76fed3beb30b44e2125))
- **vigild**: Reject invalid durations and limits ([`0f1907b`](https://github.com/git001/vigil-rs/commit/0f1907be0e3e9f8669329f7a7bf6b635221c6c7b))
- **vigild**: Reject body-template syntax errors when layers load ([`8b30528`](https://github.com/git001/vigil-rs/commit/8b30528499c774885f2c58a635866662feb47650))
- Reject unknown values of --log-format ([`1304948`](https://github.com/git001/vigil-rs/commit/1304948e1b15f16c8fe2a8b450de1c292e574bec))
- **relay**: Log to stderr and write each --stdout-sink line once ([`b4b0b43`](https://github.com/git001/vigil-rs/commit/b4b0b434454cf8a01f2d47805ff345b58cabf933))
- **vigild**: Alert env: values under the pass-env policy, prepared once when loading ([`87335a3`](https://github.com/git001/vigil-rs/commit/87335a305aaaa2e5c9d6065e5b3112ff56bd2b82))
- One tls: and proxy: block for HTTP checks and alerts ([`7d7f69f`](https://github.com/git001/vigil-rs/commit/7d7f69fa32028aaeaed73d391d9bc2c5d35955d5))
- Identities in layer files ([`cb729d4`](https://github.com/git001/vigil-rs/commit/cb729d460b44b595dfb06a49066888a4ace06e43))
- **vigild**: Reject unknown dependencies, dependency cycles and checks without one probe ([`3ccb3b1`](https://github.com/git001/vigil-rs/commit/3ccb3b1771d5a8d56de851a43e7d1b6fc892718c))
- **vigild**: Replan restarts running services whose config changed ([`0f74781`](https://github.com/git001/vigil-rs/commit/0f74781b3dd4b82b36c0ddc212effa3e28df91de))
- **vigild**: Check metrics get a level label; vigil_health, outages and time down ([`cedb36e`](https://github.com/git001/vigil-rs/commit/cedb36ef64ee3888f1e43f13599dd0dda2e33b42))
- **vigild**: Reject services without a command at load time ([`ceea10b`](https://github.com/git001/vigil-rs/commit/ceea10b295051eb2e8aff9fec66b19cff4b62386))
- **api**: Reject unknown query parameters on /v1/logs/follow ([`6d5e80c`](https://github.com/git001/vigil-rs/commit/6d5e80c702829841b7a8e30f0d0d6dbbe08b6dc2))
- **api**: Check access before the request body, errors as envelope ([`089ea88`](https://github.com/git001/vigil-rs/commit/089ea886a889fd32181a7e174892958575337f0a))
- **api**: Reject unknown query parameters on the list endpoints ([`eac191e`](https://github.com/git001/vigil-rs/commit/eac191eac500b32ee219c931547b435bdac3c91e))
- **api**: Reject unknown fields in every request body ([`8f6c6c2`](https://github.com/git001/vigil-rs/commit/8f6c6c2ac5b88aca0667896a78926c5cc4029fe3))
- Stack traces as one log entry (logs-multiline, vigil-log-relay --multiline); any output read; line length limits ([`7dea061`](https://github.com/git001/vigil-rs/commit/7dea061483032e01f9ca7bef67691a410738afc3))
- Reject a path prefix too long for notify sockets at load ([`a6fa28f`](https://github.com/git001/vigil-rs/commit/a6fa28f7868f21d67d37a2de8c69b058b6b3fc3f))
- Rename --dest-read-timeout to --dest-write-timeout ([`60fc79c`](https://github.com/git001/vigil-rs/commit/60fc79ca8ca8eda13c3d31df6a437bb436b555ec))
- **vigild**: Shutdown waits for the services, in the start order reversed ([`14e74b7`](https://github.com/git001/vigil-rs/commit/14e74b7a57bdf59f53711514adc778b46844df33))
- Service actions answer once they are done ([`b861b7d`](https://github.com/git001/vigil-rs/commit/b861b7d8ac76ecf679e7e17509b6e41318da7b9e))

### Added

- **⚠ breaking** **vigild**: Require a group for a user-id without passwd entry ([`2c26d5a`](https://github.com/git001/vigil-rs/commit/2c26d5a58d41335b415fdb247dd4ff98ac7235a9))
- **⚠ breaking** Add --path-prefix as the single runtime directory ([`c59e292`](https://github.com/git001/vigil-rs/commit/c59e292b5624cd5dfa69f1545d571075a9479111))
  - *Migration:* vigild fails to start if the path prefix (default /run/vigil) is unusable, even when --socket points elsewhere. Setups that only set --socket/VIGIL_SOCKET to a path outside /run/vigil must set --path-prefix/VIGIL_PATH_PREFIX instead.
- **⚠ breaking** **vigild**: Stop passing vigild's environment to services by default ([`8cbad53`](https://github.com/git001/vigil-rs/commit/8cbad5366df0405dac10955b543358296151e50e))
  - *Migration:* services and exec checks no longer see vigild's environment, e.g. variables set via Kubernetes env/envFrom. List the needed ones per service with pass-env-glob/pass-env-regex, or restore the old behaviour with `pass-env: true`.
- **⚠ breaking** **vigild**: No anonymous admin over TLS, make /v1/checks open ([`6adfe40`](https://github.com/git001/vigil-rs/commit/6adfe405b6dc9c512ec12ff0740e03be38a390cd))
  - *Migration:* without identities, the TLS API no longer grants admin. Anonymous TLS callers can read /v1/checks, /v1/system-info and (with the default `metrics`) /v1/metrics only. Add a `basic` or `tls` identity for remote control (e.g. `vigil --url ...`) and for log shipping (`read`); create the first identity over the Unix socket. Unix-socket callers other than root and vigild's UID are no longer admin before the first identity exists.
- **vigil-log-relay**: Authenticate to the source with basic auth or mTLS ([`cbe6c32`](https://github.com/git001/vigil-rs/commit/cbe6c3270a3bc1a9d4b3bda54e957bc41162ec27))
- **⚠ breaking** Propagate the service's exit code from vigild ([`80e3f6c`](https://github.com/git001/vigil-rs/commit/80e3f6c1e1beaf426015bf7858ab08dc82336814))
  - *Migration:* vigild's own exit code on `shutdown` and `failure-shutdown` is now the service's exit code (or 128 + signal) instead of 0/10. Start vigild with `--exit-code fixed` (or set VIGIL_EXIT_CODE=fixed) to keep the previous codes.
- **⚠ breaking** Readiness via sd_notify or checks (ready: process|notify|check) ([`4fc0b64`](https://github.com/git001/vigil-rs/commit/4fc0b6497aed599909530d92cf0673bbeb391ce9))
  - *Migration:* the services API has a new status `starting` for services that run but are not ready yet; API clients must accept it. after/requires/before now wait for `active` instead of a spawned process. vigild creates a missing path prefix and its notify/ subdirectory with mode 0711 instead of 0700. No config change is needed; the default `ready: process` behaves as in v3.
- **⚠ breaking** Oneshot services and cron schedules (type: oneshot, schedule) ([`b11a4a5`](https://github.com/git001/vigil-rs/commit/b11a4a5e55cc45b32922ee24bc1fc3262352656e))
  - *Migration:* the services API has a new status `done` for oneshot services that finished successfully and a new field `next-run`; API clients must accept both. Services ordered after a oneshot treat `done` as ready. replan now passes changed service configs to existing services (effective from their next start) and does not restart finished oneshots. No config change is needed. `schedule-tz` needs the system time zone database (Alpine: apk add tzdata).
- **⚠ breaking** Templates and env-file for services (minijinja) ([`01888d7`](https://github.com/git001/vigil-rs/commit/01888d7aee38c9c30e9bb462f4cedeb4aeea3d01))
  - *Migration:* values of a service's command and environment, and of an exec check's environment, that contain {{ or {% are now rendered as minijinja templates on every start/run; an undefined variable fails the start. Values without these markers are unchanged. To keep a literal {{, wrap it in {% raw %}…{% endraw %}.
- Watch-files restarts or signals services when their files change ([`47659c9`](https://github.com/git001/vigil-rs/commit/47659c9d68bec10eeced1a1768d69f8955ef94b5))
- Alerts for services and vigild (on-service, on-vigild, healthy-time) ([`bd5d1c4`](https://github.com/git001/vigil-rs/commit/bd5d1c4387bfb0349ac933d896f5d4d1914b64a3))
- --watch-layers reloads changed layers automatically ([`32c8485`](https://github.com/git001/vigil-rs/commit/32c8485248333a910c90c9f04cad6e71647c7de4))
- Bind TLS and healthcheck addresses at startup, allow port 0 ([`3d2969a`](https://github.com/git001/vigil-rs/commit/3d2969afde73bcad6dae98cd9ea60078afabb537))
- **⚠ breaking** One tls: and proxy: block for HTTP checks and alerts ([`7d7f69f`](https://github.com/git001/vigil-rs/commit/7d7f69fa32028aaeaed73d391d9bc2c5d35955d5))
  - *Migration:* HTTP checks and alerts take their TLS and proxy settings from a tls: block (ca, cert, key, insecure) and a proxy: block (url, no-proxy). Migration: - check ca, alert tls-ca and proxy-ca → tls.ca (one path or a list) - check insecure, alert tls-insecure → tls.insecure - check insecure-proxy → removed; put the proxy's CA into tls.ca - alert proxy and no-proxy → proxy.url and proxy.no-proxy - paths in tls: must be absolute - HTTP checks without proxy: no longer use HTTPS_PROXY from vigild's   environment; set proxy.url
- **⚠ breaking** Identities in layer files ([`cb729d4`](https://github.com/git001/vigil-rs/commit/cb729d460b44b595dfb06a49066888a4ace06e43))
  - *Migration:* the REST API rejects unknown keys in an identity's local object instead of ignoring them. Migration: - remove keys other than user-id from local in POST /v1/identities
- **examples**: Vector-push, Vector via logs-push-socket ([`1f236f9`](https://github.com/git001/vigil-rs/commit/1f236f92556679f5a2822a99c33d959d429c8a70))
- **vigild**: Report why a check fails (log, last-error, alert message) ([`f9ea8de`](https://github.com/git001/vigil-rs/commit/f9ea8def4387669829c6d16f205e77c3e2f0eebc))
- **vigild**: Run a check now (POST /v1/checks/refresh, vigil checks run) ([`23e0f1a`](https://github.com/git001/vigil-rs/commit/23e0f1a77cf661bd29c6e9c1542c088f37d4b371))
- **vigild**: GET /v1/health for Kubernetes probes ([`60dbc58`](https://github.com/git001/vigil-rs/commit/60dbc587ea07f16317d98f919713328619569dd2))
- **vigild**: Warn when an alive check probes another host ([`cb6d0e4`](https://github.com/git001/vigil-rs/commit/cb6d0e42c4c0c0b61ff924041255282331566a9e))
- **vigild**: Send a signal to single services (POST /v1/signals, vigil services signal) ([`c6db481`](https://github.com/git001/vigil-rs/commit/c6db481d3cb21f21f2eeb3ccfc45996be38f8517))
- **vigild**: Follow=true on GET /v1/logs streams NDJSON; unknown parameters are 400 ([`4b8069a`](https://github.com/git001/vigil-rs/commit/4b8069ac3a654b903c6bf7b737039464863ce5de))
- **⚠ breaking** **vigild**: Check metrics get a level label; vigil_health, outages and time down ([`cedb36e`](https://github.com/git001/vigil-rs/commit/cedb36ef64ee3888f1e43f13599dd0dda2e33b42))
  - *Migration:* vigil_check_up, vigil_check_success_count_total and vigil_check_failure_count_total have a second label, level ("alive" or "ready"), so they are new series. Selectors such as vigil_check_up{check="db"} still match; recording rules and joins that list the labels (on(check), group_left, by(check) into a stored series) need level added or ignored. A check whose level changes, or that a replan removes or disables, loses its series.
- **vigild**: Check duration and connect time histograms ([`3896a57`](https://github.com/git001/vigil-rs/commit/3896a573364bb12dfa210650b47f406fee470ae9))
- **vigil**: VIGIL_INSECURE for --insecure ([`defdb7f`](https://github.com/git001/vigil-rs/commit/defdb7fb9be2a3c108bf7a412f86f12d085e261c))
- **relay,vigil**: Env vars for all relay options, vigil logs --lines ([`c0f9f78`](https://github.com/git001/vigil-rs/commit/c0f9f78477c9131976df10038357e11adbe24b4e))
- **checks**: List checks with startup: disabled as inactive ([`bf652fb`](https://github.com/git001/vigil-rs/commit/bf652fb903531e800f8614e2a477e49428593d17))
- **⚠ breaking** Stack traces as one log entry (logs-multiline, vigil-log-relay --multiline); any output read; line length limits ([`7dea061`](https://github.com/git001/vigil-rs/commit/7dea061483032e01f9ca7bef67691a410738afc3))
  - *Migration:* a line of a service's output longer than 128 KiB is now stored and forwarded as several log entries, and vigil-log-relay forwards a pod log line or record longer than its limit in several records; before, a line of any length was one entry. Raise vigild --log-max-line-bytes (up to 1 MiB) or the relay's --max-line-bytes (up to 64 MiB) where longer lines must stay whole.
- **vigild**: --shutdown-delay: report not ready, then stop ([`58d8f4c`](https://github.com/git001/vigil-rs/commit/58d8f4cd72e8ea48e86fb596d7edef840a0d275c))
- **relay**: --send-buffer-lines; dropped lines summed up in one warning per second ([`5af6d39`](https://github.com/git001/vigil-rs/commit/5af6d391f2f4bed3efbf8ee1d56c27b3e98857d7))
- **⚠ breaking** Service actions answer once they are done ([`b861b7d`](https://github.com/git001/vigil-rs/commit/b861b7d8ac76ecf679e7e17509b6e41318da7b9e))
  - *Migration:* POST /v1/services (vigil services start/stop/restart) now answers when the action is done: start/restart/autostart once the services are ready (a oneshot once its run has ended), stop once the processes are gone; before, it answered once the command was sent. A request can take up to ready-timeout, kill-delay or the run of a oneshot: give API clients a timeout to match. Meanwhile the change is `doing`. vigil services exits with code 1 when the change ends in `error` (before: 0).
- **vigild**: Run-timeout for oneshot runs ([`b23a9b3`](https://github.com/git001/vigil-rs/commit/b23a9b3add8c63619db417a0f8dd4942e5a231df))
- GET /v1/plan and vigil plan, secrets masked ([`9cc2bbe`](https://github.com/git001/vigil-rs/commit/9cc2bbe6c6da188168d4152ac244f215458eb97b))

### Changed

- Compare configs with PartialEq, share layer merge helpers ([`ab3195e`](https://github.com/git001/vigil-rs/commit/ab3195e2c08fdbf99969412e077c02fc21b94d60))
- **vigild**: Register metrics with one helper ([`b2df9f3`](https://github.com/git001/vigil-rs/commit/b2df9f369de9b9b9b70eee0cbdb87f1239153555))
- **vigild**: Validate services, checks and alerts in own functions ([`5dbeeba`](https://github.com/git001/vigil-rs/commit/5dbeeba1d45782d165d1b3353a68c2a96a0d01fb))
- **vigild**: Share alert event JSON, render templates in one step ([`dc31469`](https://github.com/git001/vigil-rs/commit/dc31469d69f5f68a829f14fc2c1bb1946f1305af))
- **vigild**: File_io module for timed reads and content hashes ([`41a1728`](https://github.com/git001/vigil-rs/commit/41a1728a2db7a43441bf70a1858199e002fa4fa5))
- **vigild**: One helper for the prefix and notify directories ([`2b4e570`](https://github.com/git001/vigil-rs/commit/2b4e5705bedc22b804b1bd27dbccb307902a4e9b))
- Small cleanups ([`2188a2b`](https://github.com/git001/vigil-rs/commit/2188a2bf3b9cfbee4f0ff59839744f526e4d65ad))
- **vigild**: Service actor deadlines as fields, shared start/stop ([`7100ba9`](https://github.com/git001/vigil-rs/commit/7100ba993fd4169426035a453200b42c93325f86))
- Vigil-files and vigil-tls crates for all file and TLS loading ([`fe6be4f`](https://github.com/git001/vigil-rs/commit/fe6be4f8b3aeeb3e84cf2fa92d5e1878caec7218))
- **vigild**: Layer loading and validation in overlord/plan/ ([`11b4722`](https://github.com/git001/vigil-rs/commit/11b4722e81da6c6b06fce03c618c520c06ff4d9a))
- **vigil-log-relay**: Option groups in cli/, one keepalive for source and sink ([`77b1107`](https://github.com/git001/vigil-rs/commit/77b1107d27c070d52e081b22337f97159c3dde67))
- **vigil-log-relay**: Source_k8s in pods, stream, reader and selector ([`73cc078`](https://github.com/git001/vigil-rs/commit/73cc078d7b01afe00bca0b654f5c8264fad15bea))
- **vigild**: Overlord lifecycle split into sync, autostart and lifecycle ([`b6f7da2`](https://github.com/git001/vigil-rs/commit/b6f7da25c746356804a05ea460b58bf6f0ffd5fe))
- **vigild**: Split main.rs into cli, signals and main ([`a3adf08`](https://github.com/git001/vigil-rs/commit/a3adf082f70e69697a85a9fb4b1bb5b0558a6cfa))
- **vigild**: Overlord handlers split into dispatch, actions and queries ([`66816de`](https://github.com/git001/vigil-rs/commit/66816ded13b4016eb02aa0b3f9c0757bd30077ee))
- **vigil-types**: Plan/service split into config, options and watch_files ([`599cebd`](https://github.com/git001/vigil-rs/commit/599cebd4e8002143d5657568528a6ed8817708d8))
- **vigil-types**: Api split into service, check and mod ([`3efbaf4`](https://github.com/git001/vigil-rs/commit/3efbaf412f4d73b4dacb00dcaac27ed95264ea64))
- **vigil**: One send path for the Unix and HTTP transport ([`ab72860`](https://github.com/git001/vigil-rs/commit/ab7286097fd7460ed7844a11da7d164b6ac32c5a))
- **vigild**: Metrics grouped into service and check metrics ([`61cb14b`](https://github.com/git001/vigil-rs/commit/61cb14be69ad7a69b3907b11ece880f9c5c79d74))
- **vigild**: Check actor in run loop and checker ([`648cfb8`](https://github.com/git001/vigil-rs/commit/648cfb81c49e1ffbd5c438672f0c58b0cae3bbec))
- **vigild**: Overlord events split into service and check events ([`37b39cf`](https://github.com/git001/vigil-rs/commit/37b39cf209bc70f93a52240160b39faadbfdabe6))
- **vigild**: Service actor exit handling in exit.rs ([`9315cf6`](https://github.com/git001/vigil-rs/commit/9315cf6ccad890fcc474c23a20b607ba8af9e581))
- **vigild**: Service actor loop in run.rs ([`3e16652`](https://github.com/git001/vigil-rs/commit/3e1665230d055bd7cfa481730fb16f80c16eb57d))
- **vigild**: Overlord commands and startup out of mod.rs ([`fbdbfd9`](https://github.com/git001/vigil-rs/commit/fbdbfd9512346860bc14e6a7c459412da3ebc1a0))
- **vigild**: Alert mod.rs split into prepare and sender ([`d0049ae`](https://github.com/git001/vigil-rs/commit/d0049ae10c89a9d0b87c365ab33404d8e69ce2e9))
- **vigild**: Template resources split into expressions and cgroup ([`8e1d66f`](https://github.com/git001/vigil-rs/commit/8e1d66f708266a0f67d6771c4a00843e14d8e8ae))
- **vigild**: Api/mod.rs split into openapi, error and router ([`12fe2cf`](https://github.com/git001/vigil-rs/commit/12fe2cfd8cd169e109047f77cacf39df799808d5))
- **vigild**: Notify message out of the notify socket module ([`58720e5`](https://github.com/git001/vigil-rs/commit/58720e556059c319f8cd46efd2ba6a901477bb50))
- **vigild**: Exec probe split into command, run and output ([`f300253`](https://github.com/git001/vigil-rs/commit/f3002533e3e04ef4b7441441a9f578f695ae4811))
- **vigild**: Server split into the Unix socket and the TLS listener ([`26ddf0b`](https://github.com/git001/vigil-rs/commit/26ddf0bf0779931446c7189a666df27347494444))
- **vigil**: Command line in cli.rs ([`287053c`](https://github.com/git001/vigil-rs/commit/287053c3f8691cbf111e8f1fed109586f1ae0037))
- **vigild**: Service exit policies in service/policy.rs ([`6e35833`](https://github.com/git001/vigil-rs/commit/6e3583373d5ab71d096232f47e77c3014c22761c))
- **vigild,relay**: Two runtime workers by default instead of one per CPU ([`ec89391`](https://github.com/git001/vigil-rs/commit/ec89391044167968ce2ee7d44ee1033977b0f7a9))
- **vigild**: Run off the runtime only what may block; cap the blocking pool ([`14e0725`](https://github.com/git001/vigil-rs/commit/14e0725b4add2c3f8ff53430419bee06d05bdfa8))
- Move parse_duration to vigil-types ([`5723003`](https://github.com/git001/vigil-rs/commit/57230031f3c0ca3a815b2bca292ec06cfaebdac5))
- **⚠ breaking** Rename --dest-read-timeout to --dest-write-timeout ([`60fc79c`](https://github.com/git001/vigil-rs/commit/60fc79ca8ca8eda13c3d31df6a437bb436b555ec))
  - *Migration:* vigil-log-relay's --dest-read-timeout / DEST_READ_TIMEOUT is now --dest-write-timeout / DEST_WRITE_TIMEOUT: it always was the timeout of a write to the TCP sink. The old flag is rejected, and the relay refuses to start while DEST_READ_TIMEOUT is set; rename it in the service definition or the Deployment.
- **vigild**: Main.rs into start-up and shutdown modules ([`0893be6`](https://github.com/git001/vigil-rs/commit/0893be6fd4e71121be5793eed248e279ea274fb1))
- **vigild**: Check metric labels into metrics/check/labels.rs ([`c4f94a9`](https://github.com/git001/vigil-rs/commit/c4f94a9dc5c153202c1beefde67c5aa106ea318e))
- **vigild**: Template environment and blocking helpers into modules ([`8803013`](https://github.com/git001/vigil-rs/commit/8803013821c1be01fb9302427cf1c6003c57dce3))
- **vigild**: Notify socket paths into service/notify/path.rs ([`d4bc1d1`](https://github.com/git001/vigil-rs/commit/d4bc1d1a1241f6e59b8849aa337365aa384e5690))

### Fixed

- **examples**: Keep vigil-log-relay working with pass-env isolation ([`34483d0`](https://github.com/git001/vigil-rs/commit/34483d0654d7c6005d00d558e82a7709ee9f714d))
- **vigil-log-relay**: Reject URL-only TLS/auth options in other modes ([`414c046`](https://github.com/git001/vigil-rs/commit/414c046174ebe7431769677539f64f1095520b7a))
- **vigild**: Ignore reaped statuses older than the child's spawn ([`529d051`](https://github.com/git001/vigil-rs/commit/529d051152a18443a29f8a9dddde8cb84da887bc))
- **⚠ breaking** **vigil-types**: Keep startup and level when a layer does not set them ([`a26e62e`](https://github.com/git001/vigil-rs/commit/a26e62e392c91977d369cb0a590b116b22f1f410))
  - *Migration:* a layer that merges into a service or check and does not set startup (or level for checks) now keeps the earlier value; before, it silently reset it to disabled (alive). Layers that relied on this must set startup: disabled explicitly.
- **⚠ breaking** **vigild**: Bound check period to 1s..24h ([`510baad`](https://github.com/git001/vigil-rs/commit/510baade16e410efdcf8be558954af280abc0db8))
  - *Migration:* Checks with a period below 1s or above 24h are rejected, so the layer does not load. Set at least period 1s (or at most 24h).
- **vigild**: Pass-env true with lists warns instead of failing ([`41d00c2`](https://github.com/git001/vigil-rs/commit/41d00c271114f7a09971d65b5d348de213b28593))
- **vigild**: No second process from a stale backoff timer ([`b8cb660`](https://github.com/git001/vigil-rs/commit/b8cb6600ba9cca1c2f98bd67fe39eb3aba9b86b9))
- **vigild**: Start a new backoff sequence once a service is healthy ([`f4dd5e9`](https://github.com/git001/vigil-rs/commit/f4dd5e9c4938d13e01c784f03c74057216c08ec8))
- **vigild**: Drop removed services from service metrics on replan ([`17c7311`](https://github.com/git001/vigil-rs/commit/17c7311bbacf7af259517d7ff477e7ad38e94567))
- **vigild**: Stop disabled checks and restart on service-context change ([`20b0bee`](https://github.com/git001/vigil-rs/commit/20b0bee8958d0f85bcb89d85f9041f6d2cf03fc4))
- **⚠ breaking** **vigild**: Validate alert queue limits ([`4dd10b0`](https://github.com/git001/vigil-rs/commit/4dd10b041fb1b4cf37fc827cf84659f7e9a0843d))
  - *Migration:* Layers with alerts.max-queue-depth below 1, or an alerts.max-queue-time that is not a duration or is below 1s, are rejected. Use a depth of at least 1 and a time of at least 1s.
- **vigil-types**: Keep alert format and tls-insecure in layer merges ([`151d129`](https://github.com/git001/vigil-rs/commit/151d129f4319a011613eeb179ccd61ebe0bf210d))
- **vigild**: Load the TLS certificate before starting services ([`c47fca9`](https://github.com/git001/vigil-rs/commit/c47fca9d5635ca26692ff3e4c2b2995f9bbaa76c))
- **⚠ breaking** Read layer files strictly and honour override ([`950b782`](https://github.com/git001/vigil-rs/commit/950b7820ea8959c4a1c0a76fed3beb30b44e2125))
  - *Migration:* Layer files with unknown keys no longer load. Use the kebab-case key names of the docs (send-info-fields instead of send_info_fields, retry-attempts, tls-ca, proxy-ca, no-proxy, tls-insecure, retry-backoff), move misplaced keys (e.g. timeout belongs to the check, not to http), and remove identities blocks (add identities over the API). override replace now takes effect; check layers that set it.
- **⚠ breaking** **vigild**: Reject invalid durations and limits ([`0f1907b`](https://github.com/git001/vigil-rs/commit/0f1907be0e3e9f8669329f7a7bf6b635221c6c7b))
  - *Migration:* Layers with an invalid duration in these fields, a backoff-delay of 0, a backoff-limit shorter than backoff-delay, a backoff-factor below 1.0, a check timeout of 0, a threshold or retry-attempts of 0, or an empty alert url no longer load. Fix the named value.
- **⚠ breaking** **vigild**: Reject body-template syntax errors when layers load ([`8b30528`](https://github.com/git001/vigil-rs/commit/8b30528499c774885f2c58a635866662feb47650))
  - *Migration:* Layers with a body-template that does not parse no longer load. Fix the template syntax.
- **⚠ breaking** Reject unknown values of --log-format ([`1304948`](https://github.com/git001/vigil-rs/commit/1304948e1b15f16c8fe2a8b450de1c292e574bec))
  - *Migration:* vigild and vigil-log-relay no longer start with a --log-format (VIGIL_LOG_FORMAT) other than text or json.
- **vigild**: Kill timed-out exec checks with their process group ([`b03df80`](https://github.com/git001/vigil-rs/commit/b03df805a5f43b4100540c1e26d00d1e0931709a))
- **relay**: Fall back to one combined stream when the API server rejects stream= ([`90993b4`](https://github.com/git001/vigil-rs/commit/90993b454c2de36417f118de85a5102cf920cf70))
- **⚠ breaking** **relay**: Log to stderr and write each --stdout-sink line once ([`b4b0b43`](https://github.com/git001/vigil-rs/commit/b4b0b434454cf8a01f2d47805ff345b58cabf933))
  - *Migration:* vigil-log-relay writes its log messages to stderr instead of stdout. Kubernetes and container runtimes collect both streams, so nothing changes there. Setups that read the relay logs from stdout (e.g. `vigil-log-relay ... | logger`) need to redirect stderr as well, e.g. `vigil-log-relay ... 2>&1 | logger`.
- **⚠ breaking** **vigild**: Alert env: values under the pass-env policy, prepared once when loading ([`87335a3`](https://github.com/git001/vigil-rs/commit/87335a305aaaa2e5c9d6065e5b3112ff56bd2b82))
  - *Migration:* alerts see only the vigild environment variables their pass-env-glob / pass-env-regex allow (default: PATH, HOME, USER, LOGNAME, TZ). Add the variables each alert uses, e.g. `pass-env-glob: [ALERTMANAGER_*, CLUSTER_NAME]`, or `pass-env: true`. An env:VAR that is unset or not allowed now makes the layer invalid instead of sending an empty value. `"Bearer env:TOKEN"` never worked; put the whole value into the variable and write `Authorization: "env:AUTH"`. HTTPS_PROXY / HTTP_PROXY / ALL_PROXY and NO_PROXY apply to an alert only when its pass-env-glob allows them. An unreadable tls-ca or proxy-ca or an invalid proxy URL stops loading the layers.
- **examples**: Make every example run with v4 ([`6d08627`](https://github.com/git001/vigil-rs/commit/6d08627f2f5e68ce82428500ed69cb282156f80e))
- **vigild**: Autostart starts what startup starts and leaves the plan alone ([`4f08d07`](https://github.com/git001/vigil-rs/commit/4f08d070982c45e7883315718b5927977bc35dff))
- **vigild**: After on a service autostart does not start no longer blocks forever ([`dacfcfd`](https://github.com/git001/vigil-rs/commit/dacfcfdfcff3c25aef702493792af47b445acb49))
- **⚠ breaking** **vigild**: Reject unknown dependencies, dependency cycles and checks without one probe ([`3ccb3b1`](https://github.com/git001/vigil-rs/commit/3ccb3b1771d5a8d56de851a43e7d1b6fc892718c))
  - *Migration:* layers with these mistakes no longer load. Migration: - fix the service names in after, before and requires - break dependency cycles; the error shows the chain - give each check exactly one of http, tcp or exec - point service-context at an existing service
- **⚠ breaking** **vigild**: Replan restarts running services whose config changed ([`0f74781`](https://github.com/git001/vigil-rs/commit/0f74781b3dd4b82b36c0ddc212effa3e28df91de))
  - *Migration:* a replan (vigil replan, POST /v1/replan, --watch-layers) now restarts a running service whose configuration changed in more than summary or description, and then the running services that require it. Before, the new configuration applied only from the service's next start. To keep a process running across a replan, change its configuration only together with a planned restart.
- **vigild**: Replan action of POST /v1/services loads the layers once ([`728d966`](https://github.com/git001/vigil-rs/commit/728d9662b61e10c4ec17f2df9909eb91308d819e))
- **vigild**: Warn about services blocked by a failed before dependency ([`35df6e4`](https://github.com/git001/vigil-rs/commit/35df6e4806b356a733bff8bbbdf4bba545265bec))
- **⚠ breaking** **vigild**: Reject services without a command at load time ([`ceea10b`](https://github.com/git001/vigil-rs/commit/ceea10b295051eb2e8aff9fec66b19cff4b62386))
  - *Migration:* a service that has no `command` (or an empty one) after all layers are merged is now a load error ("service "x": command is required"); vigild does not start and a replan is rejected. Before, the plan loaded and the service went to `error` only when it was started. Migration: give every service a `command` in some layer, or remove the service.
- **types**: Name the key when an alerts: entry is invalid ([`c9adf99`](https://github.com/git001/vigil-rs/commit/c9adf999d5e68da3e1631610a6e05f023d1fe3e8))
- **⚠ breaking** **api**: Reject unknown query parameters on /v1/logs/follow ([`6d5e80c`](https://github.com/git001/vigil-rs/commit/6d5e80c702829841b7a8e30f0d0d6dbbe08b6dc2))
  - *Migration:* GET /v1/logs/follow answers 400 for an unknown query parameter instead of ignoring it. Migration: use only `services` and `format` (json, text, ndjson); fix typos such as `service=`.
- **⚠ breaking** **api**: Check access before the request body, errors as envelope ([`089ea88`](https://github.com/git001/vigil-rs/commit/089ea886a889fd32181a7e174892958575337f0a))
  - *Migration:* request body errors (415 wrong Content-Type, 400 invalid JSON, 422 unknown field or wrong type) are now a JSON error envelope ({"type":"error","status-code":…,"message":…}) instead of plain text, and a caller without access gets 403 before any body error. Migration: clients that matched the plain-text body should read `message` from the envelope.
- **⚠ breaking** **api**: Reject unknown query parameters on the list endpoints ([`eac191e`](https://github.com/git001/vigil-rs/commit/eac191eac500b32ee219c931547b435bdac3c91e))
  - *Migration:* GET /v1/services, /v1/checks, /v1/alerts and /v1/identities answer 400 for a query parameter other than `names` instead of ignoring it, and query errors on /v1/health are a JSON error envelope instead of plain text. Migration: use only `names`; read `message` from the envelope.
- **api**: Document 400/415/422 of the JSON endpoints and replan errors ([`085fb1a`](https://github.com/git001/vigil-rs/commit/085fb1a2717a672f2958f7f95f51ca2790c25d6a))
- **vigil**: Identities remove requires at least one name ([`0f9e2ed`](https://github.com/git001/vigil-rs/commit/0f9e2ed61a5c9e3dc81575c5af9fa46502de847b))
- **⚠ breaking** **api**: Reject unknown fields in every request body ([`8f6c6c2`](https://github.com/git001/vigil-rs/commit/8f6c6c2ac5b88aca0667896a78926c5cc4029fe3))
  - *Migration:* a request body with a field vigild does not know is 422 on every endpoint (before: ignored on /v1/services, /v1/vigild and /v1/identities). Migration: send only the documented fields.
- **vigild,relay**: Plain pid and path values, no colours outside a terminal ([`18ad43e`](https://github.com/git001/vigil-rs/commit/18ad43e7a4eca7f6f2ba9885bd9532f849010f95))
- **api**: Query parameters are query parameters in the OpenAPI spec ([`045d05a`](https://github.com/git001/vigil-rs/commit/045d05ab6e4d28ca664915054fb6ee2f2d0bd7bd))
- **relay**: Compare pod log timestamps as points in time ([`7e0df9f`](https://github.com/git001/vigil-rs/commit/7e0df9f93e58202a5f904137ee961586713e84d6))
- **⚠ breaking** Reject a path prefix too long for notify sockets at load ([`a6fa28f`](https://github.com/git001/vigil-rs/commit/a6fa28f7868f21d67d37a2de8c69b058b6b3fc3f))
  - *Migration:* layers with a `ready: notify` service are rejected when --path-prefix (VIGIL_PATH_PREFIX) is longer than 78 bytes. Use a shorter prefix, e.g. /tmp/vigil.
- **relay**: The parts of a joined record past --max-line-bytes follow the filter decision ([`50d908a`](https://github.com/git001/vigil-rs/commit/50d908ab4efc84d671e553c7fea37bccee8caac8))
- **relay**: Keep the line when the TCP sink closed the connection ([`99f0b84`](https://github.com/git001/vigil-rs/commit/99f0b84071e1f4c2eb76382e67c14ab5d834382d))
- **relay**: --container with names streams only the containers a pod has ([`53dbbac`](https://github.com/git001/vigil-rs/commit/53dbbac425d1399ba3bef0e850d623b2f0f92914))
- **vigild**: --watch-layers no longer misses a change made while the layers load ([`095b2a4`](https://github.com/git001/vigil-rs/commit/095b2a46b3126c4f62e9cb38544c9655d3eb84fa))
- **⚠ breaking** **vigild**: Shutdown waits for the services, in the start order reversed ([`14e74b7`](https://github.com/git001/vigil-rs/commit/14e74b7a57bdf59f53711514adc778b46844df33))
  - *Migration:* on SIGTERM, `vigil vigild stop` or a shutdown policy, vigild now stops a service only after the services that depend on it (after, requires, before) have exited, and waits for each up to its kill-delay before SIGKILL; before, it signalled all at once and exited at once, leaving the services to be killed with the container. A shutdown can take up to the sum of the kill-delays along the longest dependency chain: keep terminationGracePeriodSeconds above --shutdown-delay plus that sum.

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

[4.0.1]: https://github.com/git001/vigil-rs/compare/v4.0.0...v4.0.1
[4.0.0]: https://github.com/git001/vigil-rs/compare/v3.2.2...v4.0.0
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

