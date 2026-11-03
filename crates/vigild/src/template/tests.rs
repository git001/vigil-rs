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
