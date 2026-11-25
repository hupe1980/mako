//! An attempted second open must leave the incumbent SlateDB writer usable.

use std::{
    net::{SocketAddr, TcpListener},
    path::Path,
    process::Stdio,
    time::Duration,
};

use reqwest::Client;
use serde_json::{Value, json};
use tokio::{process::Command, time::timeout};

const KEY: &str = "0123456789abcdef0123456789abcdef";

fn makod(cfg: &Path, dir: &Path, addr: SocketAddr) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_makod"));
    cmd.arg("--config")
        .arg(cfg)
        .arg("--data-dir")
        .arg(dir)
        .args(["--http-addr", &addr.to_string()])
        .args(["--auth-key", &format!("erp-prod={KEY}")])
        .args([
            "--worker-threads",
            "2",
            "--allow-no-as4-signing",
            "--cedar-permit-all",
        ])
        .kill_on_drop(true);
    cmd
}

async fn start(cmd: &mut Command, http: &Client, addr: SocketAddr) -> tokio::process::Child {
    let mut child = cmd.stdout(Stdio::null()).spawn().unwrap();
    let ready = format!("http://{addr}/health/ready");
    timeout(Duration::from_secs(30), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "makod exited before readiness"
            );
            if http
                .get(&ready)
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("makod reached readiness");
    child
}

#[tokio::test]
async fn local_writer_excludes_daemon_check_and_migration_then_recovers_an_acknowledged_write() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("store");
    let guard = makod::startup::lock_data_dir(&real).unwrap();
    #[cfg(unix)]
    let alias = {
        let alias = tmp.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        alias
    };
    #[cfg(not(unix))]
    let alias = real.join(".");
    let cfg = tmp.path().join("makod.toml");
    std::fs::write(
        &cfg,
        r#"
[deployment]
tenant = "local-writer-fixture"
rollen = ["nb-strom"]
[[party]]
mp_id = "9900001000001"
roles = ["NB"]
primary = true
"#,
    )
    .unwrap();
    let addr = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    for args in [&[][..], &["--check"], &["migrate"]] {
        let out = timeout(
            Duration::from_secs(20),
            makod(&cfg, &alias, addr).args(args).output(),
        )
        .await
        .expect("a competing open must refuse promptly")
        .unwrap();
        let reason = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success() && reason.contains("already using the data directory"),
            "{reason}"
        );
        assert!(!real.join("db").exists(), "a refused open touched SlateDB");
    }
    let report = makod(&cfg, &alias, addr)
        .args(["--check-config", "--json"])
        .output()
        .await
        .unwrap();
    assert!(report.status.success() && serde_json::from_slice::<Value>(&report.stdout).is_ok());
    assert!(!real.join("db").exists());
    drop(guard);

    let http = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let mut writer = start(&mut makod(&cfg, &real, addr), &http, addr).await;
    let rival = makod(&cfg, &alias, addr)
        .arg("--check")
        .output()
        .await
        .unwrap();
    assert!(!rival.status.success());
    let mp = "9900357000003";
    let record = format!("http://{addr}/admin/partners/{mp}");
    http.put(&record)
        .bearer_auth(KEY)
        .json(&json!({"mp_id":mp,"display_name":"Local writer reference"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    writer.kill().await.unwrap();
    let mut replacement = start(&mut makod(&cfg, &alias, addr), &http, addr).await;
    let recovered: Value = http
        .get(&record)
        .bearer_auth(KEY)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recovered["display_name"], "Local writer reference");
    replacement.kill().await.unwrap();
    for arg in ["migrate", "--check"] {
        let out = makod(&cfg, &real, addr).arg(arg).output().await.unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_redirected_db_cannot_bypass_its_directory_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let first = makod::startup::lock_data_dir(&tmp.path().join("first")).unwrap();
    let second = makod::startup::lock_data_dir(&tmp.path().join("second")).unwrap();
    let real = first.path().join("db");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, second.path().join("db")).unwrap();
    let err = second
        .open_store()
        .await
        .err()
        .expect("redirected db must refuse to open");
    assert!(err.to_string().contains("inside the locked data directory"));
    assert!(std::fs::read_dir(real).unwrap().next().is_none());
}
