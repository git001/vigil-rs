// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::collections::BTreeMap;

use super::*;

fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn render(s: &str, v: &[(&str, &str)]) -> anyhow::Result<String> {
    Renderer::new().render(s, &vars(v))
}

#[test]
fn plain_values_are_untouched() {
    for s in [
        "",
        "plain",
        "${#ARR[@]}",
        "a {# b",
        "100%",
        "x }} y",
        "trailing\n",
    ] {
        assert_eq!(render(s, &[]).unwrap(), s);
    }
}

#[test]
fn variables_and_defaults() {
    let v = [("PORT", "9090"), ("EMPTY", "")];
    assert_eq!(render("{{ env.PORT }}", &v).unwrap(), "9090");
    assert_eq!(
        render("{{ env.MISSING | default('8080') }}", &v).unwrap(),
        "8080"
    );
    // Jinja semantics: default only applies to undefined values...
    assert_eq!(render("{{ env.EMPTY | default('x') }}", &v).unwrap(), "");
    // ...unless asked for empty values too.
    assert_eq!(
        render("{{ env.EMPTY | default('x', true) }}", &v).unwrap(),
        "x"
    );
    assert_eq!(
        render("{% if env.MISSING is defined %}a{% else %}b{% endif %}", &v).unwrap(),
        "b"
    );
    assert_eq!(render("{{ env['PORT'] }}", &v).unwrap(), "9090");
}

#[test]
fn undefined_variable_is_an_error() {
    let err = render("--port={{ env.PORT }}", &[]).unwrap_err();
    assert!(format!("{err:#}").contains("undefined"), "{err:#}");
}

#[test]
fn substituted_values_are_not_rendered_again() {
    let v = [("X", "{{ env.Y }}"), ("Y", "secret")];
    assert_eq!(render("{{ env.X }}", &v).unwrap(), "{{ env.Y }}");
    let v = [("X", "{{ file('/etc/passwd') }}")];
    assert_eq!(
        render("a{{ env.X }}b", &v).unwrap(),
        "a{{ file('/etc/passwd') }}b"
    );
}

#[test]
fn trailing_newline_is_kept_and_raw_works() {
    assert_eq!(render("{{ 'a' }}\n", &[]).unwrap(), "a\n");
    assert_eq!(
        render("{% raw %}{{ x }} ${#Y}{% endraw %}", &[]).unwrap(),
        "{{ x }} ${#Y}"
    );
}

#[test]
fn no_html_escaping() {
    let v = [("X", "<a & 'b'>")];
    assert_eq!(render("{{ env.X }}", &v).unwrap(), "<a & 'b'>");
}

#[test]
fn file_function() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    std::fs::write(&secret, "s3cr3t\n").unwrap();
    let p = secret.display();

    assert_eq!(
        render(&format!("{{{{ file('{p}') }}}}"), &[]).unwrap(),
        "s3cr3t"
    );

    // Kubernetes secret volume: key -> ..data/key, ..data -> ..2026_x
    let real = dir.path().join("..2026_x");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("token"), "k8s\n\n").unwrap();
    std::os::unix::fs::symlink("..2026_x", dir.path().join("..data")).unwrap();
    std::os::unix::fs::symlink("..data/token", dir.path().join("token")).unwrap();
    let t = dir.path().join("token");
    assert_eq!(
        render(&format!("{{{{ file('{}') }}}}", t.display()), &[]).unwrap(),
        "k8s\n",
        "only one trailing newline is removed"
    );

    let missing = dir.path().join("nope");
    assert_eq!(
        render(
            &format!("{{{{ file('{}', 'dflt') }}}}", missing.display()),
            &[]
        )
        .unwrap(),
        "dflt"
    );
    let err = render(&format!("{{{{ file('{}') }}}}", missing.display()), &[]).unwrap_err();
    assert!(format!("{err:#}").contains("No such file"), "{err:#}");
}

#[test]
fn file_function_rejects_bad_files() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("big"), vec![b'a'; 64 * 1024 + 1]).unwrap();
    std::fs::write(d.join("max"), vec![b'a'; 64 * 1024]).unwrap();
    std::fs::write(d.join("nul"), b"a\0b").unwrap();
    std::fs::write(d.join("bin"), [0xff, 0xfe]).unwrap();

    let r = |name: &str| {
        render(
            &format!("{{{{ file('{}') }}}}", d.join(name).display()),
            &[],
        )
    };
    assert_eq!(r("max").unwrap().len(), 64 * 1024);
    for (name, msg) in [
        ("big", "larger than 64 KiB"),
        ("nul", "NUL"),
        ("bin", "UTF-8"),
        ("", "not a regular file"), // the directory itself
    ] {
        let err = r(name).unwrap_err();
        assert!(format!("{err:#}").contains(msg), "{name}: {err:#}");
    }
    // a directory is an error even with a default
    let err = render(&format!("{{{{ file('{}', 'x') }}}}", d.display()), &[]).unwrap_err();
    assert!(format!("{err:#}").contains("not a regular file"));
    let err = render("{{ file('relative/path') }}", &[]).unwrap_err();
    assert!(format!("{err:#}").contains("absolute"), "{err:#}");
}

#[test]
fn mem_and_cpu_render_numbers() {
    let cpus: u64 = render("{{ cpu() }}", &[]).unwrap().parse().unwrap();
    assert!(cpus >= 1);
    let half: u64 = render("{{ cpu('50%') }}", &[]).unwrap().parse().unwrap();
    assert!(half >= 1 && half <= cpus);
    let mib: u64 = render("{{ mem('50%') }}", &[]).unwrap().parse().unwrap();
    assert!(mib > 0);
    assert_eq!(render("{{ mem('256MiB') }}MiB", &[]).unwrap(), "256MiB");
    assert!(render("{{ mem('lots') }}", &[]).is_err());
}

fn svc(command: &str, env: &[(&str, &str)]) -> vigil_types::plan::ServiceConfig {
    vigil_types::plan::ServiceConfig {
        command: Some(command.into()),
        environment: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..Default::default()
    }
}

#[test]
fn check_service_reports_syntax_errors_only() {
    // valid, even though PORT is not defined (resolved at start)
    check_service(&svc(
        r#"app --port "{{ env.PORT | default("80") }}" -x"#,
        &[("A", "{{ file('/x') }}"), ("B", "${#ARR[@]}")],
    ))
    .unwrap();

    let err = check_service(&svc("app {{ env.PORT ", &[])).unwrap_err();
    assert!(format!("{err:#}").contains("unterminated"), "{err:#}");
    let err = check_service(&svc("app {{ env.PORT | }}", &[])).unwrap_err();
    assert!(format!("{err:#}").contains("command"), "{err:#}");
    let err = check_service(&svc("app", &[("B", "{{ x }} ${#ARR}")])).unwrap_err();
    assert!(format!("{err:#}").contains("environment \"B\""), "{err:#}");
}

#[test]
fn empty_file_is_an_error_unless_a_default_is_given() {
    let dir = tempfile::tempdir().unwrap();
    for (name, content) in [("empty", ""), ("newline", "\n")] {
        let p = dir.path().join(name);
        std::fs::write(&p, content).unwrap();
        let err = render(&format!("{{{{ file('{}') }}}}", p.display()), &[]).unwrap_err();
        assert!(format!("{err:#}").contains("is empty"), "{name}: {err:#}");
        assert_eq!(
            render(&format!("{{{{ file('{}', 'fb') }}}}", p.display()), &[]).unwrap(),
            "fb",
            "{name}"
        );
    }
    // two newlines: one is removed, "\n" remains, which is not empty
    let p = dir.path().join("two");
    std::fs::write(&p, "\n\n").unwrap();
    assert_eq!(
        render(&format!("{{{{ file('{}') }}}}", p.display()), &[]).unwrap(),
        "\n"
    );
}

#[test]
fn empty_file_is_recorded_for_watching() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("empty");
    std::fs::write(&p, "").unwrap();
    let r = Renderer::new();
    assert!(
        r.render(
            &format!("{{{{ file('{}', 'x') }}}}", p.display()),
            &BTreeMap::new()
        )
        .is_ok()
    );
    let reads = r.take_reads();
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].hash, Some(content_hash(b"")), "exists, empty");
}

/// A fallback *file*: `file(a, '') or file(b)`. `or` short-circuits, so `b`
/// is only read (and only required) when `a` is missing or empty.
#[test]
fn fallback_file_via_or() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    let tmpl = format!(
        "{{{{ file('{}', '') or file('{}') }}}}",
        a.display(),
        b.display()
    );

    // a exists: b is neither needed nor read
    std::fs::write(&a, "from-a\n").unwrap();
    let r = Renderer::new();
    assert_eq!(r.render(&tmpl, &BTreeMap::new()).unwrap(), "from-a");
    assert_eq!(r.take_reads().len(), 1, "b not read");

    // a empty: b is used
    std::fs::write(&a, "").unwrap();
    std::fs::write(&b, "from-b").unwrap();
    assert_eq!(render(&tmpl, &[]).unwrap(), "from-b");

    // both missing/empty: error from b (no default there)
    std::fs::remove_file(&b).unwrap();
    assert!(render(&tmpl, &[]).is_err());

    // chain with a final literal
    let chain = format!(
        "{{{{ file('{}', '') or file('{}', '') or 'last-resort' }}}}",
        a.display(),
        b.display()
    );
    assert_eq!(render(&chain, &[]).unwrap(), "last-resort");
}

/// Fallbacks are returned exactly as written, `=`, spaces and quotes included.
#[test]
fn fallback_values_are_returned_verbatim() {
    let missing = "/nonexistent/vigil-test/log-flag";
    for fb in ["--loglevel=info", "DB_PASSWORD=MY-pw1!", "--opt='a b' x=1"] {
        assert_eq!(
            render(&format!("{{{{ file('{missing}', {fb:?}) }}}}"), &[]).unwrap(),
            fb
        );
        assert_eq!(
            render(&format!("{{{{ env.LOG_FLAG | default({fb:?}) }}}}"), &[]).unwrap(),
            fb
        );
        assert_eq!(
            render(&format!("{{{{ file('{missing}', '') or {fb:?} }}}}"), &[]).unwrap(),
            fb
        );
    }
    let v = [("LOG_FLAG", "--loglevel=debug")];
    assert_eq!(
        render("{{ env.LOG_FLAG | default('--loglevel=info') }}", &v).unwrap(),
        "--loglevel=debug"
    );
}

#[test]
fn may_block_only_for_files_templates_or_a_user_switch() {
    use super::may_block;
    let none: [&str; 0] = [];
    assert!(!may_block(&[], ["plain", "--port=8080"], false));
    assert!(!may_block(&[], none, false));
    assert!(may_block(&["/vault/secrets/db".into()], none, false));
    assert!(may_block(&[], ["{{ file('/x') }}"], false));
    assert!(may_block(
        &[],
        ["{% if env.A is defined %}-v{% endif %}"],
        false
    ));
    assert!(may_block(&[], none, true));
}
