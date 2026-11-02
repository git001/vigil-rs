# Plan vigil-rs v4: Security-Fixes und neue Features

Stand: 2026-11-02, Basis v3.2.1 (`4d78710`).
Grundlage: Feature-Vergleich mit gopherd sowie dessen Commit `75e38f4`
("close trust-boundary gaps between PID 1 and services").

**Ziel-Version: v4.0.** Wegen der neuen Features und der geänderten Defaults
(`pass-env`, neuer Status `Starting`, Exit-Codes) wird das ein Major-Release.

## Grundsatz: Alles muss unter OpenShift laufen

Unter OpenShift (`restricted-v2` SCC) gilt:

- vigild läuft mit einer zufälligen UID (z.B. `1000680000`) und GID 0.
- Es gibt keine Capabilities, also kein `CAP_SETUID` oder `CAP_SETGID`.
- `/run` ist ein tmpfs, das root gehört und nicht beschreibbar ist
  (siehe `docs/operator-guide/orchestrators.md`, dort wird bereits
  `--socket /tmp/vigild.sock` verwendet).
- Mit `readOnlyRootFilesystem` ist nur beschreibbar, was als `emptyDir` gemountet ist.

Daraus folgt für alle neuen Dateien und Sockets:

- **Ein Verzeichnis für alles: `--path-prefix` / `VIGIL_PATH_PREFIX`.**
  Alles, was vigild zur Laufzeit anlegt, kommt dort hinein, nie fest nach `/run`.
  Es gibt keine Fallback-Kette, der Pfad ist explizit.
  - Default: `/run/vigil`. Unter OpenShift z.B. `/data/my-vigil` oder `/tmp/vigil`.
  - Inhalt:
    ```
    <prefix>/vigild.sock          Control-Socket (Default für --socket)
    <prefix>/notify/<svc>-<rand>.sock   sd_notify (Phase 2)
    ```
  - `--socket` bleibt als Override bestehen. Der Default ist neu
    `<prefix>/vigild.sock`, das gilt auch für den Client `vigil`.
- **Prüfung beim Start, bei Fehler Abbruch mit klarer Meldung:**
  - Das Verzeichnis muss existieren und darf kein Symlink sein (`lstat`).
  - Owner ist die eigene euid.
  - Nicht beschreibbar für other (kein `o+w`). `g+w` ist erlaubt, weil unter
    OpenShift Verzeichnisse oft `chmod g=u` für GID 0 haben.
  - Beschreibbar für vigild (Test über `access(W_OK)`).
  - Die Fehlermeldung nennt Pfad, gefundenen Owner und Modus und die
    erwarteten Werte.
- **Unterverzeichnisse** wie `notify/` legt vigild selbst mit 0700 an.
- **Letzte Komponente anlegen (entschieden):** Fehlt die letzte Pfadkomponente
  und existiert das übergeordnete Verzeichnis, legt vigild sie selbst mit 0700
  an und prüft sie danach wie oben. Tiefere fehlende Ebenen sind ein Fehler.
  Grund: Ein `emptyDir`-Mountpunkt gehört root und hat Modus 0777, als Prefix
  direkt würde er abgelehnt. So funktioniert `emptyDir` auf `/data` mit
  `--path-prefix /data/my-vigil` ohne initContainer.
- **Keine Pflicht-Capabilities:** Kein Feature darf Capabilities voraussetzen.
  Code für den User-Wechsel läuft nur, wenn `user` oder `group` konfiguriert ist.

## Phase 0: Security-Fixes

### S1 Rechte werden beim User-Wechsel nur halb abgegeben

**Problem:** `crates/vigild/src/service/actor/spawn.rs:48-65` und
`crates/vigild/src/check/probe/exec.rs:75-90` rufen nur `setgid` und `setuid` auf.

- Die Zusatzgruppen von root bleiben erhalten, denn `setgroups` wird nirgends aufgerufen.
- Bei `user:` ohne `group:` liefert `resolve_gid` `None`
  (`crates/vigild/src/process_util/identity.rs:52`). Dann wird gar kein
  `setgid` aufgerufen und die primäre GID bleibt 0.

**Wen das betrifft:** nur Setups, in denen vigild als root läuft und `user:`
konfiguriert ist (Docker, Podman, VMs).

**OpenShift ist nicht betroffen.** Dort ist kein `user:` gesetzt, also wird
der `pre_exec`-Zweig gar nicht ausgeführt, und alle Prozesse laufen mit der
zugewiesenen UID. GID 0 ist dort so gewollt: OpenShift erwartet, dass Dateien
im Image der Gruppe 0 gehören und für sie beschreibbar sind. Wäre `user:`
gesetzt, schlägt `setuid` heute schon mit `EPERM` fehl.

**Fix:** gemeinsamer Helper `process_util::drop_privileges` für Services und Exec-Checks.

- Läuft nur, wenn `user` oder `group` konfiguriert ist. Ohne diese Angaben
  wird nie `setgroups` aufgerufen, denn das bräuchte `CAP_SETGID` und
  würde OpenShift kaputt machen.
- Ohne `group` wird die primäre GID des Users aus `getpwnam` genommen.
- Die Gruppenliste wird vor dem Fork ermittelt (`getgrouplist`). In `pre_exec`
  laufen nur `setgroups`, `setgid` und `setuid`, denn dort darf nichts allokiert werden.
- Ist die Ziel-UID gleich der eigenen euid (häufig bei Rootless), wird nichts
  geändert und es gibt keinen Fehler.
- Nach `setuid` prüfen, dass sich `setuid(0)` nicht mehr ausführen lässt.

**Tests:** Unit-Test für die GID-Auflösung. Integrationstest als root prüft
`id -G` im Kindprozess. Ein Test ohne Root und ohne `user:` stellt sicher,
dass kein Rechte-Syscall aufgerufen wird.

**Stand:** In v3.2.2 umgesetzt. Für einen numerischen `user-id` ohne
passwd-Eintrag und ohne Gruppe gibt 3.2.2 nur eine Warnung aus und nimmt
GID = UID. Auf dem Branch `v4` ist daraus ein Fehler geworden (erledigt).

### S2 Kindprozesse erben die Umgebung von vigild

**Problem:** Weder Services noch Exec-Probes rufen `env_clear()` auf. Die
Umgebung von vigild (Tokens, Passwörter) landet in jedem Kindprozess.

**Entscheidung:** `pass-env` ist per Default `false`.

**Fix:**

- Drei getrennte Felder pro Service:
  ```yaml
  pass-env: false                         # Default
  pass-env-glob: [DATABASE_URL, "APP_*", "*_APP_*_LAL"]
  pass-env-regex: ['^K8S_[A-Z]+_URL$', 'TOKEN_(A|B)']
  ```
  - `pass-env: false`: `env_clear()` plus `PATH`, `HOME`, `USER`, `LOGNAME`
    und `TZ`, dazu alles, was über `pass-env-glob` oder `pass-env-regex`
    passt, dazu `environment:`.
  - `pass-env: true`: die gesamte Umgebung von vigild. Sind zusätzlich
    `pass-env-glob` oder `pass-env-regex` gesetzt, ist das ein
    Validierungsfehler, weil sie dann wirkungslos wären.
  - `pass-env-glob`: YAML-Liste. `*` (beliebig viele Zeichen) und `?` (ein
    Zeichen) an beliebiger Stelle, der Glob muss den **ganzen** Namen treffen.
    Kleiner eigener Matcher, kein Crate nötig.
  - `pass-env-regex`: YAML-Liste (ein String mit `A|B` geht auch). Crate
    `regex`, ist schon Workspace-Dependency. Jeder Ausdruck wird als
    `^(?:…)$` verankert, damit `TOKEN` nicht versehentlich `MY_TOKEN_SECRET`
    trifft. Wer Teiltreffer will, schreibt `.*TOKEN.*`.
  - Glob- und Regex-Treffer werden vereinigt. Die Ausdrücke werden beim
    Laden des Plans kompiliert, ungültige führen zum Fehler.
  - Overlay-Merge: Listen werden angehängt (wie `after`), `pass-env` überschreibt.
  - `vigil plan` zeigt die Muster. Welche Variablen konkret durchgereicht
    werden, erscheint nur als Namen im Debug-Log, nie mit Werten.
- Exec-Checks im Kontext eines Service bekommen genau dieselbe Umgebung.
- vigild ruft beim Start `prctl(PR_SET_DUMPABLE, 0)` auf. Das braucht keine
  Capabilities und gilt nur für vigild selbst.

**Warum das unter OpenShift/K8s wichtig ist:**

- Dort laufen vigild und alle Services mit derselben UID. Ohne
  `PR_SET_DUMPABLE=0` kann jeder Service `/proc/1/environ` lesen, und
  `env_clear()` allein schützt dann nicht. Mit `PR_SET_DUMPABLE=0` gehört
  `/proc/1/*` root und ist für die gleiche UID nicht mehr lesbar.
- Variablen aus dem Deployment (`env:`, `envFrom:` aus Secrets, auch von ESO)
  bekommt zuerst vigild. Mit Default `false` sehen die Services sie **nicht
  mehr automatisch**. Dafür gibt es die Allowlist, und in der Migrationsanleitung
  für v4 muss das prominent stehen.

### S3 Der Reaper schnappt tokio die Exit-Status weg

**Problem:** `crates/vigild/src/reaper.rs:81` ruft `waitpid(-1)` bei jedem
`SIGCHLD` auf. Damit kann er auch eigene Services und Probes einsammeln, bevor
tokio sie abholt. tokio bekommt dann `ECHILD`, und
`crates/vigild/src/service/actor/handlers.rs:110-115` macht daraus
„failure, Code 1“. Ohne diesen Fix ist Phase 1 nicht zuverlässig umsetzbar.

**Fix** (so löst es auch gopherd mit `reaper.Registry`):

- Der Reaper wird der einzige Aufrufer von `wait`.
- Spawn und Registrierung laufen unter einem gemeinsamen Lock
  (`Mutex<HashMap<Pid, oneshot::Sender<WaitStatus>>>`), sonst kann ein Kind
  sterben, bevor es registriert ist.
- Registrierte PIDs bekommen ihren Status über den oneshot-Kanal, unbekannte werden verworfen.
- Der Service-Actor wartet auf diesen Kanal statt auf `Child::wait()`.
  Exec-Probes laufen über denselben Weg.

**Test:** zuerst ein Repro. Viele Services, die schnell mit `exit 3` enden, und
Orphans parallel. Dann prüfen, dass immer 3 gemeldet wird.

**Stand:** Größtenteils behoben durch PR #1 (daveman1010221), kommt in
v3.2.2. Der Reaper merkt sich eingesammelte Status 60 s lang,
`reaper::wait_child` holt sie bei `ECHILD`. Exit-Codes sind damit
zuverlässig, Phase 1 hängt nicht mehr am Registry-Umbau.

Offen für v4:

- Status ignorieren, die älter als der Spawn des Kindes sind. Sonst könnte
  bei PID-Wiederverwendung innerhalb von 60 s ein alter Orphan-Status
  geliefert werden. Machen wir in v4 selbst.
- Registry-Umbau (s.o.) nur noch wegen der Signale: Zwischen dem Einsammeln
  durch den Reaper und der Verarbeitung im Actor hält tokio die PID noch für
  gültig, `kill` könnte eine neu vergebene PID treffen.

### S4 Control-Socket absichern

- **Server** (`crates/vigild/src/server.rs:53-63`):
  - Mit `lstat` prüfen und Symlinks ablehnen.
  - Mit umask `0177` binden, danach explizit `chmod 0600`.
  - Lässt sich eine fremde Datei am Pfad nicht entfernen, bricht vigild ab, statt nur zu warnen.
  - In v4 liegt der Socket unter `--path-prefix` und wird mit dessen Regeln
    geprüft. Im Backport 3.2.2 gibt es keinen Prefix, dort nur die Prüfungen
    am Socket selbst.
- **Client** (`vigil`, Unix-Transport): Vor dem Senden mit `SO_PEERCRED`
  prüfen, dass die Gegenseite root ist oder die eigene euid hat.
  Unter OpenShift ist das die gleiche zufällige UID, das passt.

**Stand:** In v3.2.2 umgesetzt (`bind_unix`, `--socket-mode` mit Default
600, `vigil --socket-uid`). Auf dem Branch `v4` erledigt: `--path-prefix` mit
Prüfung des Verzeichnisses (Modul `crates/vigild/src/path_prefix.rs`).

Beim Smoke-Test gefunden und auf `v4` behoben: Der temporäre Socket-Name aus
3.2.2 (`.vigild.sock.<pid>.tmp`) ist länger als der endgültige. Liegt der Pfad
nahe an der Grenze von 107 Byte, scheitert der Bind. Jetzt `.<pid hex>.tmp`
und eine klare Fehlermeldung bei zu langen Pfaden.

### S5 Ohne Identities ist jeder Aufrufer Admin, auch über TLS

**Problem:** `crates/vigild/src/api/auth/mod.rs:92` gibt jedem Aufrufer
Admin-Rechte, solange keine Identity hinterlegt ist. Das gilt auch für den
TLS-Listener über TCP: Mit `--tls-addr` ohne Identities kann jeder, der den
Port erreicht, alle Services steuern.

**Entscheidung:** Fix erst in v4, kein Backport.

**Fix:**

- Ohne Identities bekommt nur der Unix-Socket Admin, und zwar nur, wenn die
  Peer-UID root oder die euid von vigild ist (wie bei pebble).
- Über TLS gibt es ohne Identities nur Zugriff auf Endpunkte ohne Auth
  (`Open`), z.B. Health und Metrics.
- Beim Start mit `--tls-addr`, aber ohne Identities, gibt vigild eine Warnung
  aus, dass die TLS-API nur lesend erreichbar ist.
- Migrationsanleitung: Wer heute die TLS-API ohne Identities nutzt, braucht in
  v4 eine `basic`- oder `tls`-Identity.

### Nicht betroffen

`O_CLOEXEC` und das Pipe-Leak aus dem gopherd-Commit betreffen vigil-rs nicht.
Rust-`std` und tokio erledigen das schon. Die neuen Features unten müssen das beibehalten.

## Phase 1: Echte Exit-Codes, braucht S3

- `EventKind::ProcessExited` bekommt `ExitInfo { code: Option<i32>, signal: Option<i32> }`.
- Neue Daemon-Option `exit-code: fixed | propagate`. Bei `propagate` beendet
  sich vigild mit dem Code des Service, bei einem Signal mit `128+sig`.
  **Default in v4: `propagate`.** `fixed` (das alte 0/10) bleibt für die Migration.
- Optional pro Service: `success-exit-codes: [0, 143]` und `exit-code-map: {143: 0}`.
- Die Pfade `stop_all()` + `process::exit` an
  `crates/vigild/src/overlord/events.rs:131-144` und
  `crates/vigild/src/overlord/mod.rs:199-204` werden zu einer Funktion
  `shutdown(code)` zusammengefasst.
- Nebenbei: letzter Exit-Code in API und Status, Metrik
  `vigil_service_last_exit_code`, `vigil services -o json`.

## Phase 2: Readiness und sd_notify

- **Ausgangslage:** `Starting` dauert nur einen Moment, denn
  `crates/vigild/src/service/actor/spawn.rs:111` setzt sofort `Active`.
  `after_deps_running` wartet nur auf `is_running()`. `CheckLevel::Ready`
  wird angezeigt, aber für nichts genutzt.
- **Neue Felder pro Service:** `ready: process | notify | check` (Default
  `process`), `ready-check: <name>`, `ready-timeout: 30s`.
- **Zustände:** `Starting` bleibt bestehen, bis die Readiness erfüllt ist,
  dann kommt `Active`. Abhängige Services warten auf `Active`. Beim Timeout
  greift die `on-failure`-Policy.
- **sd_notify, ohne zusätzliches Crate:**
  - Pro Service ein `UnixDatagram` unter
    `<path-prefix>/notify/<svc>-<64bit random>.sock`. Modus 0600, Owner ist die
    Service-UID, ohne User-Wechsel also die eigene UID, `chown` entfällt dann.
    Ein Pfad im Dateisystem statt eines abstrakten Namens vermeidet das
    Squatting-Problem, das gopherd hatte.
  - Der Pfad kommt in `NOTIFY_SOCKET`. Das gilt auch bei `pass-env: false`.
  - Empfang über `recvmsg` mit `SCM_CREDENTIALS`. Akzeptiert werden nur
    Nachrichten von PIDs in der Prozessgruppe des Service.
  - Ausgewertet werden `READY=1`, `STATUS=` (erscheint in der API) und
    `STOPPING=1`. Watchdog kommt später.
- **API (Breaking, deshalb v4):** `ServiceStatus` bekommt `Starting`.
  OpenAPI-Version auf v4 anheben. `vigil` v4 und `vigil-log-relay` müssen
  den neuen Wert kennen.

## Phase 3: Oneshot und Cron, baut auf Phase 2 auf

- `type: simple | oneshot`
  - Ein Oneshot gilt mit Exit 0 als „ready“, abhängige Services starten danach.
  - Nach Erfolg kein Neustart. Ein Fehler läuft über die normale
    `on-failure`-Policy mit Backoff.
  - Neuer Endzustand `Done` (ebenfalls neu in der API).
- `schedule: "*/5 * * * *"`, nur für `oneshot`:
  - Crate `croner` mit Feature `jiff` (Vorgabe des Users), also `jiff` statt
    `chrono` für Zeitzonen und die Berechnung des nächsten Termins.
    5 oder 6 Felder, korrekter Umgang mit der Sommerzeit. Vor der Umsetzung
    prüfen, welche croner-Version das Feature anbietet.
  - Zusätzlicher `sleep_until(next)`-Zweig im `select!` des Actors.
  - Überlappende Läufe: Default `skip`, wenn der vorige Lauf noch aktiv ist.
  - Zeitzone UTC, optional `schedule-tz`.
  - Hot-Reload berechnet den nächsten Termin neu.
  - Kein Zustand auf der Platte (kein „letzter Lauf“-File). Verpasste Termine
    während eines Pod-Neustarts werden nicht nachgeholt.
- Metriken und Alerts: Ergebnis und Zeitpunkt des letzten Laufs.

## Phase 4: Templating

- **Syntax:** `{{.VAR}}`, `{{.VAR:-default}}`, `{{file "/p"}}`, `{{mem 75%}}`, `{{cpu 50%}}`.
- **Wo:** `command` und die Werte von `environment`.
- **Wann:** Beim Laden des Plans nur Syntaxprüfung. Aufgelöst wird erst beim
  Spawn. Bei jedem (Neu-)Start eines Service wird `{{file}}` also frisch gelesen.
- **Parser:** neues Modul `crates/vigild/src/template/`, handgeschrieben, ohne
  Regex oder Template-Engine. Alles in einem einzigen Durchgang über den
  Originaltext (das war der Template-Bug bei gopherd). Fuzzing mit proptest.
- **`file`:** nur reguläre Dateien, maximal 64 KiB, abschließender Zeilenumbruch
  wird entfernt. Aufgelöste Werte erscheinen nie in Logs oder in der Plan-API.
  - Symlinks müssen erlaubt sein: Kubernetes-Secret-Volumes bestehen aus
    Symlinks (`key -> ..data/key`, `..data -> ..2026_11_02_...`). Kein
    `O_NOFOLLOW` auf den Pfad, aber nach dem Öffnen mit `fstat` prüfen, dass
    es eine reguläre Datei ist.
- **`mem` / `cpu`:** cgroup v2 `memory.max` bzw. `cpu.max`, sonst
  `/proc/meminfo` bzw. `sched_getaffinity`. Unter OpenShift liefern diese
  Werte die Pod-Limits.
- **`.VAR`:** Quelle ist `environment` des Service plus die per `pass-env`
  freigegebenen Variablen von vigild.

### Phase 4b: Dateien beobachten (Ersatz für stakater Reloader)

**Ausgangslage mit ESO:** ESO aktualisiert das Kubernetes-Secret. Was dann passiert:

- **Secret als Volume (ohne `subPath`):** Das Kubelet aktualisiert die Datei im
  Pod nach etwa 1-2 Minuten (Sync-Periode plus Cache). Ein `{{file}}` liest
  den neuen Wert beim nächsten Start des Service.
- **Secret als Env-Variable (`env`/`envFrom`):** wird nie aktualisiert, nur
  durch einen Neustart des Pods. Dafür braucht man weiter Reloader.
- **Volume mit `subPath`:** wird nie aktualisiert.

Phase 4 allein liest den neuen Wert also nur, wenn der Service aus einem
anderen Grund neu startet. Um Reloader zu ersetzen, braucht es einen Auslöser:

- **Neues Feld pro Service:**
  ```yaml
  watch-files:
    paths: [/etc/secrets/db-password]   # zusätzlich zu allen {{file}}-Pfaden
    on-change: restart                  # restart | signal:SIGHUP | ignore
    interval: 30s
  ```
- **Erkennung per Polling** (Hash bzw. Inode und mtime alle `interval`), nicht
  per inotify. Grund: Das Kubelet tauscht den `..data`-Symlink atomar aus,
  inotify auf die Datei bekommt das nicht zuverlässig mit. Polling ist einfach
  und funktioniert auch auf NFS und overlayfs.
- **Debounce:** ESO und Kubelet aktualisieren mehrere Keys nacheinander. Erst
  reagieren, wenn sich der Inhalt eine Polling-Runde lang nicht mehr ändert.
- **Vorteil gegenüber Reloader:** Es wird nur der betroffene Service neu
  gestartet, nicht der ganze Pod. Bei `signal:SIGHUP` gar nichts, wenn die
  Anwendung Reload kann.
- Ein Event pro Änderung geht an das Alerting und erscheint als Metrik
  `vigil_service_file_reloads_total`.

### Phase 4c: Layers aus einer ConfigMap, automatisches `replan`

**Ziel:** Statt eines vigil-Operators (bewusst verworfen: würde Kubernetes
nachbauen, braucht CRD, Controller, RBAC und API-Zugriff) reicht der
typische Anwendungsfall: Layers als ConfigMap mounten, und vigild übernimmt
Änderungen selbst. Damit entfällt `kubectl exec … vigil replan` nach einem
Update der ConfigMap.

- **Neue Option** für vigild: `--watch-layers <interval>` /
  `VIGIL_WATCH_LAYERS` (z.B. `30s`, Default aus).
- **Erkennung per Polling** wie in Phase 4b, mit derselben Code-Basis:
  Hash über Namen und Inhalt aller `*.yaml` im Layer-Verzeichnis. Grund wie
  dort: Das Kubelet tauscht bei ConfigMaps den `..data`-Symlink atomar aus.
  Dateien, die mit `..` beginnen (`..data`, `..2026_…`), werden ignoriert.
- **Debounce:** erst reagieren, wenn der Hash eine Polling-Runde lang stabil ist.
- **Validierung vor dem Replan:** Die neuen Layers erst parsen und prüfen.
  Bei einem Fehler bleibt der laufende Plan aktiv, es gibt ein Log und ein
  Alert-Event (`layers-invalid`), und es wird nicht erneut versucht, bis
  sich der Inhalt wieder ändert.
- **Replan** nutzt die bestehende Logik von `vigil replan`, also nur
  geänderte Services und Checks neu starten.
- Metriken: `vigil_layers_reload_total{result="ok|invalid"}`, Zeitpunkt des
  letzten erfolgreichen Reloads.
- Doku in `orchestrators.md`: ConfigMap-Beispiel ohne `subPath` (mit
  `subPath` aktualisiert das Kubelet nie).

## Releases

```
S1, S4 (Backport, ohne path-prefix) ──────────► v3.2.2

S1, S2, S4, S5, prefix ──┐
S3-Rest, Phase 1 (Exit) ┤
Phase 2 (Readiness) ─────┼──► v4.0
Phase 3 (Oneshot/Cron) ──┤
Phase 4 + 4b + 4c ───────┘    (Phase 4 nach S2)
```

Empfohlene Reihenfolge der Arbeit: 3.2.2-Backport zuerst, dann auf `main`
S1-Fehlerfall → S4 + path-prefix → S5 → S2 → S3 → Phase 1 → Phase 2 → Phase 3 → Phase 4 → 4b → 4c.

Für jede Phase: Unit-Tests, VTest2-Integrationstests nach bestehendem Muster,
ein Test unter OpenShift-Bedingungen (zufällige UID, GID 0, `/run` nicht
beschreibbar, `readOnlyRootFilesystem` mit `emptyDir` auf `/tmp`), Updates in
`docs/user-guide/configuration.md`, `docs/operator-guide/orchestrators.md` und
`docs/specs/openapi.yaml`, Coverage nicht unter 92 %. Dazu eine
Migrationsanleitung v3 → v4.

## Entschieden

- Ziel-Version v4.0, **alle Phasen kommen in v4.0**.
- S1 und S4 zusätzlich als Backport in v3.2.2.
- `pass-env` Default `false`, Allowlists über getrennte Felder
  `pass-env-glob` und `pass-env-regex`.
- Exit-Codes: `propagate` ist Default in v4.
- Ein einziges Verzeichnis `--path-prefix` für alle Laufzeitdateien, Prüfung
  beim Start, bei Fehler Abbruch.
- Alles muss unter OpenShift ohne Schreibrechte auf `/run` und ohne Capabilities laufen.
- Neuer Status `Starting` und Änderungen an der OpenAPI kommen in v4.

- vigild legt die letzte Komponente von `--path-prefix` selbst mit 0700 an,
  wenn sie fehlt.

## Offene Entscheidungen

Keine.
