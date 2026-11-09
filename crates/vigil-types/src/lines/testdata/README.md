# Real stack traces

Output of real runtimes, used by `lines/tests.rs` to check the multi-line
presets: each file is a log line, a stack trace, and one more log line, so
the preset must give exactly two entries.

- `java-*.log`: Eclipse Temurin 21 (`docker.io/library/eclipse-temurin:21-jdk`),
  a small program that logs an exception with `printStackTrace` between two
  log lines: 200 frames deep, chained (`Caused by:`, `Suppressed:`,
  `... N more`), from a stream lambda, and a `StackOverflowError` (1024
  frames, the JVM's default limit, 33 KB).
- `python-*.log`: CPython 3.13, `logging.exception()`: `raise … from …`,
  an exception during handling of another, and an `ExceptionGroup`. The
  script path is replaced by `/app/pytrace.py`.
