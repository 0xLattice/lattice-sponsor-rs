use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
};
#[test]
fn help_is_available_without_files_or_node() {
    let output = Command::new(env!("CARGO_BIN_EXE_lattice-sponsor"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("--share-work"));
    assert!(text.contains("--serving-rate"));
}
#[tokio::test]
async fn real_cli_binds_and_serves_terms_with_no_available_work() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mock = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let node = format!("http://{}", mock.local_addr().unwrap());
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = mock.accept().await.unwrap();
            let mut bytes = [0; 4096];
            let n = stream.read(&mut bytes).await.unwrap();
            if n == 0 {
                continue;
            }
            let request = String::from_utf8_lossy(&bytes[..n]);
            let (status, body) = if request.starts_with("GET /hosted-chains ") {
                (200, "{\"chains\":[]}")
            } else {
                (503, "{}")
            };
            let reply = format!(
                "HTTP/1.1 {status} Reply\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let key = temp.path().join("key.json");
    std::fs::write(&key,r#"{"privateKey":"9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60","publicKey":"ed01d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"}"#).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_lattice-sponsor"))
        .args(["--node", &node, "--cookie-file"])
        .arg(temp.path().join("cookie"))
        .arg("--key")
        .arg(&key)
        .args(["--port", "0"])
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (first, _stderr) = tokio::task::spawn_blocking(move || {
        let mut line = String::new();
        let mut reader = BufReader::new(stderr);
        reader.read_line(&mut line).unwrap();
        (line, reader)
    })
    .await
    .unwrap();
    let address = first.trim().split(" listening on ").nth(1).unwrap();
    let response: reqwest::Response = reqwest::get(format!("http://{address}/terms"))
        .await
        .unwrap();
    assert!(response.status().is_success());
    let terms: serde_json::Value = response.json().await.unwrap();
    assert_eq!(terms["depth"], "6");
    child.kill().unwrap();
    child.wait().unwrap();
    task.abort();
    let _ = task.await;
}
