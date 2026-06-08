//! SSH tester. v1 is a TCP-connect probe — opens a socket to the
//! SSH port, reads the server's version banner, closes. That
//! verifies the host is reachable and is speaking SSH; it doesn't
//! verify the credentials. A future revision can do a real key
//! handshake using `russh` or similar; for v1 the operator's
//! "credentials work" feedback comes when they run the first
//! actual command through SSH.
//!
//! v1 deliberately does NOT submit the password / private key over
//! the wire as part of the test — pulling in a full SSH crypto
//! stack just to validate creds would balloon the dep tree.

use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;

pub async fn test(fields: &Value) -> Result<String, String> {
    let host = fields
        .get("host")
        .and_then(|v| v.as_str())
        .ok_or("missing field: host")?;
    let port = fields.get("port").and_then(|v| v.as_u64()).unwrap_or(22) as u16;
    let addr = format!("{host}:{port}");
    let stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr))
        .await
        .map_err(|_| format!("connect timeout to {addr}"))?
        .map_err(|err| format!("tcp connect {addr}: {err}"))?;

    let mut reader = BufReader::new(stream);
    let mut banner = String::new();
    let read_result =
        tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut banner)).await;
    let banner = match read_result {
        Ok(Ok(_n)) => banner.trim().to_string(),
        Ok(Err(err)) => return Err(format!("read banner: {err}")),
        Err(_) => return Err("banner read timeout".into()),
    };

    if !banner.starts_with("SSH-") {
        return Err(format!(
            "host returned `{banner}` — does not look like an SSH server"
        ));
    }
    Ok(format!(
        "reached {addr}; server banner: {banner}. Note: credentials NOT verified at v1; first real ssh.* command will exercise them."
    ))
}
