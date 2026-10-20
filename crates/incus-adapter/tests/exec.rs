// SPDX-License-Identifier: AGPL-3.0-only
//! Interactive sessions over real mutual-TLS handshakes and real websocket upgrades against a
//! local server that speaks Incus's exec protocol as documented (an asynchronous exec operation
//! whose metadata carries one secret per channel; the data and control sockets of the operation
//! are then opened with those secrets). Evidence: CONTRACT VERIFIED; not a real Incus server.

// Test fixtures: a failed `unwrap` is the failure.
#![allow(clippy::unwrap_used, clippy::result_large_err)]

mod common;

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use apelora_incus_adapter::{
    AdapterError, IncusClient, Provider, ShellInput, ShellRequest, ShellSession,
};
use common::{Pem, cfg, incus_like};
use futures_util::{SinkExt, StreamExt};
use rustls::RootCertStore;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::WebPkiClientVerifier;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

const OP: &str = "11111111-2222-4333-8444-555555555555";
const DATA_SECRET: &str = "data-secret-do-not-log";
const CONTROL_SECRET: &str = "control-secret-do-not-log";

/// A stream that first replays bytes already read from it.
struct Replay<S> {
    head: Vec<u8>,
    inner: S,
}

impl<S: AsyncRead + Unpin> AsyncRead for Replay<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if !self.head.is_empty() {
            let n = self.head.len().min(buf.remaining());
            let rest = self.head.split_off(n);
            buf.put_slice(&std::mem::replace(&mut self.head, rest));
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Replay<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[derive(Default)]
struct Seen {
    /// Bodies of the exec calls.
    exec: Vec<Value>,
    /// Requests that were not websockets: "METHOD path".
    http: Vec<String>,
    /// Messages received on the control socket.
    control: Vec<Value>,
}

#[derive(Clone)]
struct Fixture {
    status: &'static str,
    managed: bool,
    /// Does the fixture's exec call hand out the right secret for the data socket?
    data_secret_ok: bool,
}

fn envelope(kind: &str, metadata: Value) -> String {
    let extra = if kind == "async" {
        format!(r#","operation":"/1.0/operations/{OP}""#)
    } else {
        String::new()
    };
    format!(
        r#"{{"type":"{kind}","status":"Success","status_code":200,"error_code":0,"error":""{extra},"metadata":{metadata}}}"#
    )
}

async fn respond(tls: &mut (impl AsyncWrite + Unpin), body: &str) {
    let r = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = tls.write_all(r.as_bytes()).await;
    let _ = tls.shutdown().await;
}

async fn serve_one(
    mut tls: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    fx: Fixture,
    seen: Arc<Mutex<Seen>>,
) {
    // Read the request head (and the body of a POST).
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = tls.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let first = head.lines().next().unwrap_or_default().to_owned();
    let mut parts = first.split(' ');
    let (method, path) = (
        parts.next().unwrap_or_default().to_owned(),
        parts.next().unwrap_or_default().to_owned(),
    );
    if head.to_lowercase().contains("upgrade: websocket") {
        let is_data = path.contains(&format!("secret={DATA_SECRET}"));
        let is_control = path.contains(&format!("secret={CONTROL_SECRET}"));
        let allowed = path.starts_with(&format!("/1.0/operations/{OP}/websocket?"))
            && ((is_data && fx.data_secret_ok) || is_control);
        let stream = Replay {
            head: buf,
            inner: tls,
        };
        let check = |_: &Request, resp: Response| -> Result<Response, ErrorResponse> {
            if allowed {
                Ok(resp)
            } else {
                Err(ErrorResponse::new(Some("forbidden".into())))
            }
        };
        let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, check).await else {
            return;
        };
        if is_control {
            while let Some(Ok(m)) = ws.next().await {
                if let Message::Text(t) = m
                    && let Ok(v) = serde_json::from_str::<Value>(&t)
                {
                    seen.lock().unwrap().control.push(v);
                }
            }
        } else {
            let _ = ws.send(Message::Binary(b"$ ".to_vec().into())).await;
            while let Some(Ok(m)) = ws.next().await {
                match m {
                    Message::Binary(b) if b.as_ref() == b"exit\r" => {
                        let _ = ws
                            .send(Message::Binary(b"logout\r\n".to_vec().into()))
                            .await;
                        let _ = ws.close(None).await;
                        return;
                    }
                    Message::Binary(b) => {
                        let mut out = b"echo:".to_vec();
                        out.extend_from_slice(&b);
                        let _ = ws.send(Message::Binary(out.into())).await;
                    }
                    Message::Close(_) => return,
                    _ => {}
                }
            }
        }
        return;
    }
    // Plain request.
    let content_length = head
        .lines()
        .find_map(|l| {
            l.to_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < head_end + content_length {
        let n = tls.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    seen.lock().unwrap().http.push(format!("{method} {path}"));
    let body = if method == "GET" && path.starts_with("/1.0/instances/web-01") {
        envelope(
            "sync",
            json!({
                "name": "web-01", "status": fx.status, "architecture": "x86_64",
                "config": if fx.managed { json!({"user.apelora.managed": "true"}) } else { json!({}) },
            }),
        )
    } else if method == "POST" && path.starts_with("/1.0/instances/web-01/exec") {
        let v: Value = serde_json::from_slice(&buf[head_end..head_end + content_length])
            .unwrap_or(Value::Null);
        seen.lock().unwrap().exec.push(v);
        envelope(
            "async",
            json!({"id": OP, "class": "websocket", "status": "Running", "status_code": 103,
                   "metadata": {"fds": {"0": DATA_SECRET, "control": CONTROL_SECRET}}}),
        )
    } else if method == "DELETE" && path.starts_with(&format!("/1.0/operations/{OP}")) {
        envelope("sync", json!({}))
    } else {
        r#"{"type":"error","error":"not found","error_code":404,"metadata":null}"#.to_owned()
    };
    respond(&mut tls, &body).await;
}

async fn server(server: &Pem, trusted_client: &Pem, fx: Fixture) -> (u16, Arc<Mutex<Seen>>) {
    let mut roots = RootCertStore::empty();
    roots.add(trusted_client.der.clone()).expect("root");
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .expect("verifier");
    let config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![server.der.clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server.key_der.clone())),
        )
        .expect("server config");
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let shared = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let (acceptor, fx, seen) = (acceptor.clone(), fx.clone(), shared.clone());
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(tcp).await {
                    serve_one(tls, fx, seen).await;
                }
            });
        }
    });
    (port, seen)
}

fn request() -> ShellRequest {
    ShellRequest {
        project: "apelora".into(),
        name: "web-01".into(),
        command: vec!["/bin/sh".into()],
        uid: Some(1000),
        gid: Some(1000),
        cwd: Some("/tmp".into()),
        env: BTreeMap::from([("TERM".to_owned(), "xterm-256color".to_owned())]),
        cols: 100,
        rows: 30,
    }
}

async fn connect(fx: Fixture) -> (Provider, Arc<Mutex<Seen>>) {
    let srv = incus_like(&["unspecified"], false);
    let client = incus_like(&[], true);
    let (port, seen) = server(&srv, &client, fx).await;
    let c = IncusClient::mtls(&cfg(port, "127.0.0.1", &client, &srv, None)).expect("client");
    (Provider::Incus(c), seen)
}

fn running() -> Fixture {
    Fixture {
        status: "Running",
        managed: true,
        data_secret_ok: true,
    }
}

async fn next_output(s: &mut ShellSession) -> Option<String> {
    tokio::time::timeout(Duration::from_secs(5), s.output.recv())
        .await
        .expect("output in time")
        .map(|b| String::from_utf8_lossy(&b).to_string())
}

#[tokio::test]
async fn a_session_carries_typed_bytes_and_output_over_the_instances_exec_sockets() {
    let (p, seen) = connect(running()).await;
    let mut s = p.open_shell(&request()).await.expect("session");
    assert_eq!(next_output(&mut s).await.as_deref(), Some("$ "));
    s.input
        .send(ShellInput::Data(b"ls\r".to_vec()))
        .await
        .unwrap();
    assert_eq!(next_output(&mut s).await.as_deref(), Some("echo:ls\r"));
    s.input
        .send(ShellInput::Resize {
            cols: 120,
            rows: 40,
        })
        .await
        .unwrap();
    s.input
        .send(ShellInput::Data(b"exit\r".to_vec()))
        .await
        .unwrap();
    assert_eq!(next_output(&mut s).await.as_deref(), Some("logout\r\n"));
    assert_eq!(
        next_output(&mut s).await,
        None,
        "the output ends with the process"
    );

    let seen = seen.lock().unwrap();
    let body = &seen.exec[0];
    assert_eq!(body["command"], json!(["/bin/sh"]));
    assert_eq!(body["interactive"], json!(true));
    assert_eq!(body["wait-for-websocket"], json!(true));
    assert_eq!(body["user"], json!(1000), "not root");
    assert_eq!(body["group"], json!(1000));
    assert_eq!(body["width"], json!(100));
    assert_eq!(body["height"], json!(30));
    assert_eq!(body["record-output"], json!(false));
    assert_eq!(body["environment"]["TERM"], json!("xterm-256color"));
    assert!(
        seen.control
            .contains(&json!({"command":"window-resize","args":{"width":"120","height":"40"}})),
        "{:?}",
        seen.control
    );
}

#[tokio::test]
async fn a_request_without_a_user_sends_none() {
    let (p, seen) = connect(running()).await;
    let mut r = request();
    (r.uid, r.gid, r.cwd) = (None, None, None);
    let mut s = p.open_shell(&r).await.expect("session");
    assert_eq!(next_output(&mut s).await.as_deref(), Some("$ "));
    let seen = seen.lock().unwrap();
    for key in ["user", "group", "cwd"] {
        assert!(seen.exec[0].get(key).is_none(), "{key} must be absent");
    }
}

#[tokio::test]
async fn dropping_the_session_hangs_up() {
    let (p, seen) = connect(running()).await;
    let mut s = p.open_shell(&request()).await.expect("session");
    assert_eq!(next_output(&mut s).await.as_deref(), Some("$ "));
    drop(s);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        seen.lock()
            .unwrap()
            .control
            .contains(&json!({"command":"signal","signal":1})),
        "a consumer that goes away ends the process"
    );
}

#[tokio::test]
async fn hanging_up_sends_a_hangup_signal_on_the_control_socket() {
    let (p, seen) = connect(running()).await;
    let mut s = p.open_shell(&request()).await.expect("session");
    assert_eq!(next_output(&mut s).await.as_deref(), Some("$ "));
    s.input.send(ShellInput::Hangup).await.unwrap();
    assert_eq!(next_output(&mut s).await, None);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        seen.lock()
            .unwrap()
            .control
            .contains(&json!({"command":"signal","signal":1}))
    );
}

#[tokio::test]
async fn a_stopped_or_foreign_instance_is_refused_before_any_exec_call() {
    for fx in [
        Fixture {
            status: "Stopped",
            ..running()
        },
        Fixture {
            managed: false,
            ..running()
        },
    ] {
        let (p, seen) = connect(fx).await;
        let err = p.open_shell(&request()).await.expect_err("refused");
        assert!(
            matches!(err, AdapterError::Api { status: 409, .. }),
            "{err:?}"
        );
        assert!(seen.lock().unwrap().exec.is_empty(), "nothing was started");
    }
}

#[tokio::test]
async fn a_refused_websocket_cancels_the_operation_and_never_leaks_the_secret() {
    let (p, seen) = connect(Fixture {
        data_secret_ok: false,
        ..running()
    })
    .await;
    let err = p.open_shell(&request()).await.expect_err("refused");
    let text = format!("{err:?} {err}");
    assert!(matches!(err, AdapterError::Connectivity(_)), "{text}");
    assert!(
        !text.contains(DATA_SECRET) && !text.contains(CONTROL_SECRET),
        "{text}"
    );
    assert!(
        seen.lock()
            .unwrap()
            .http
            .iter()
            .any(|r| r == &format!("DELETE /1.0/operations/{OP}")),
        "the unused exec operation is cancelled"
    );
}

#[tokio::test]
async fn an_untrusted_server_certificate_is_not_accepted_for_sessions_either() {
    let srv = incus_like(&["unspecified"], false);
    let impostor_pin = incus_like(&["unspecified"], false);
    let client = incus_like(&[], true);
    let (port, seen) = server(&srv, &client, running()).await;
    let c =
        IncusClient::mtls(&cfg(port, "127.0.0.1", &client, &impostor_pin, None)).expect("client");
    let err = Provider::Incus(c)
        .open_shell(&request())
        .await
        .expect_err("pinned certificate mismatch");
    assert!(matches!(err, AdapterError::Connectivity(_)), "{err:?}");
    assert!(seen.lock().unwrap().exec.is_empty());
}

#[tokio::test]
async fn the_mock_provider_runs_a_labelled_fake_shell_and_refuses_unknown_instances() {
    let p = Provider::Mock(apelora_incus_adapter::MockIncus::default());
    let err = p
        .open_shell(&request())
        .await
        .expect_err("no such instance");
    assert!(matches!(err, AdapterError::NotFound(_)), "{err:?}");
}
