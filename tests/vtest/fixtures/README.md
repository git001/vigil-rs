# vtest fixtures

Static files used by the `.vtc` tests (referenced as `${vtestdir}/fixtures/…`).
They are plain vigil layers and request bodies, so they double as examples.

## vNNNN/

Layer files of test `vNNNN_*.vtc`; the test copies them into its layers
directory (`cp ${vtestdir}/fixtures/v0005/demo.yaml ${tmpdir}/layers/`) and
runs `vigil replan`.

Values known only at run time (a port, the test's tmpdir) are not written
into the file by the test; the layer gets them the way a real deployment
would:

| Need | How | Example |
|---|---|---|
| A URL in an alert | `url: "env:WEBHOOK_URL"`, vigild started with `WEBHOOK_URL=…` | `v0016`, `v0020` |
| A path in an exec check | `test -f "$CHECK_MARKER"` plus `pass-env-glob: [CHECK_MARKER]`, vigild started with `CHECK_MARKER=…` | `v0012`, `v0016` |
| A field without `env:` support | placeholder `@SINK_PORT@`, filled in with `sed` | `v0015` (`logs-push-addr`) |

## identities/

Request bodies for `POST /v1/identities` with test-only basic-auth users:

| File | User | Password | Access | Hash created with |
|---|---|---|---|---|
| `admin-user.json` | `admin-user` | `admpass` | admin | `openssl passwd -6 -salt testhttps admpass` |
| `extra-user.json` | `extra-user` | `extrapass` | read | `openssl passwd -6 -salt testextra extrapass` |

Use them as an example of how to create an identity over the Unix socket:

```sh
curl --unix-socket /run/vigil/vigild.sock \
  -X POST http://localhost/v1/identities \
  -H "Content-Type: application/json" \
  -d @tests/vtest/fixtures/identities/admin-user.json
```
