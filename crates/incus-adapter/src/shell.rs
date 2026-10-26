// SPDX-License-Identifier: AGPL-3.0-only
//! Sessions inside an instance through Incus exec: with a terminal (a shell, PowerShell), or
//! without one for a byte stream that must pass unchanged (the remote desktop relay). The adapter
//! hands the control plane two bounded channels; the browser never sees an Incus address,
//! certificate or secret. The mock provider answers with a labelled fake shell (or, without a
//! terminal, echoes the bytes back) and touches nothing.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use reqwest::Method;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::client::IncusClient;
use crate::{AdapterError, MockIncus};

/// Largest chunk of output or input handed over at once.
pub const MAX_CHUNK: usize = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A send that takes longer than this means the other side has stopped reading: end the session
/// rather than wait, so timers and limits keep working.
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// What to start in which instance, and as whom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellRequest {
    pub project: String,
    pub name: String,
    /// The program and its arguments (e.g. `["/bin/sh"]`).
    pub command: Vec<String>,
    /// Numeric user and group inside the instance (Linux guests). Not root unless the operator
    /// chose so. `None` sends no user, for guests where numeric ids mean nothing.
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    pub cols: u16,
    pub rows: u16,
    /// With a terminal. Without one, standard input and output are separate channels that carry
    /// bytes unchanged, and the size is ignored.
    pub interactive: bool,
}

/// What the person typed or did to the terminal window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellInput {
    Data(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    /// End the session: the process gets a hang-up.
    Hangup,
}

/// An open session. `output` ends (returns `None`) when the process exits or the channel breaks.
#[derive(Debug)]
pub struct ShellSession {
    pub input: mpsc::Sender<ShellInput>,
    pub output: mpsc::Receiver<Vec<u8>>,
}

impl ShellRequest {
    fn check(&self) -> Result<(), AdapterError> {
        let bad = |m: &str| Err(AdapterError::InvalidConfig(m.to_owned()));
        if !apelora_domain::is_valid_name(&self.project)
            || !apelora_domain::is_valid_name(&self.name)
        {
            return bad("invalid project or instance name");
        }
        if self.command.is_empty() || self.command.iter().any(|c| c.is_empty() || c.len() > 1024) {
            return bad("a session needs a command");
        }
        if self.interactive
            && (self.cols == 0 || self.rows == 0 || self.cols > 1000 || self.rows > 1000)
        {
            return bad("the terminal size is out of range");
        }
        Ok(())
    }
}

// ---- mock -------------------------------------------------------------------------------------

/// The fake shell the mock provider runs: a few built-in commands, nothing executed anywhere.
#[derive(Debug)]
pub struct MockShell {
    instance: String,
    line: Vec<u8>,
}

impl MockShell {
    pub fn new(instance: &str) -> Self {
        Self {
            instance: instance.to_owned(),
            line: Vec::new(),
        }
    }

    pub fn banner(&self) -> Vec<u8> {
        format!(
            "MOCK session for {}: no real instance is behind this terminal.\r\nTry: hostname, id, exit\r\n$ ",
            self.instance
        )
        .into_bytes()
    }

    /// Feeds typed bytes; returns what to show and whether the session ended.
    pub fn feed(&mut self, bytes: &[u8]) -> (Vec<u8>, bool) {
        let mut out = Vec::new();
        for &b in bytes {
            match b {
                b'\r' | b'\n' => {
                    out.extend_from_slice(b"\r\n");
                    let cmd = String::from_utf8_lossy(&self.line).trim().to_owned();
                    self.line.clear();
                    match cmd.as_str() {
                        "" => {}
                        "exit" | "logout" => {
                            out.extend_from_slice(b"logout\r\n");
                            return (out, true);
                        }
                        "hostname" => {
                            out.extend_from_slice(format!("{}\r\n", self.instance).as_bytes())
                        }
                        "id" => out.extend_from_slice(b"uid=1000 gid=1000 (mock)\r\n"),
                        other => out.extend_from_slice(
                            format!(
                                "{}: command not found (mock shell)\r\n",
                                other.split(' ').next().unwrap_or("")
                            )
                            .as_bytes(),
                        ),
                    }
                    out.extend_from_slice(b"$ ");
                }
                0x03 => {
                    self.line.clear();
                    out.extend_from_slice(b"^C\r\n$ ");
                }
                0x04 if self.line.is_empty() => {
                    out.extend_from_slice(b"logout\r\n");
                    return (out, true);
                }
                0x7f | 0x08 => {
                    if self.line.pop().is_some() {
                        out.extend_from_slice(b"\x08 \x08");
                    }
                }
                0x20..=0x7e => {
                    self.line.push(b);
                    out.push(b);
                }
                _ => {}
            }
        }
        (out, false)
    }
}

impl MockIncus {
    /// A labelled fake shell for a running, managed instance; refuses anything else the way
    /// the real adapter does.
    pub fn open_shell(&self, req: &ShellRequest) -> Result<ShellSession, AdapterError> {
        req.check()?;
        let found = self
            .instances
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|i| i.instance.project == req.project && i.instance.name == req.name)
            .map(|i| (i.instance.managed, i.instance.status.clone()));
        match found {
            None => Err(AdapterError::NotFound(format!(
                "instance {}/{}",
                req.project, req.name
            ))),
            Some((false, _)) => Err(AdapterError::Api {
                status: 409,
                message: "the instance is not managed by Apelora".into(),
            }),
            Some((true, status)) if status != "Running" => Err(AdapterError::Api {
                status: 409,
                message: format!("the instance is {}", status.to_lowercase()),
            }),
            Some(_) => {
                let (in_tx, mut in_rx) = mpsc::channel::<ShellInput>(32);
                let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(64);
                if !req.interactive {
                    // No terminal: hand every byte back unchanged, so a relay can be tested
                    // end to end without a guest.
                    tokio::spawn(async move {
                        while let Some(ShellInput::Data(d)) = in_rx.recv().await {
                            if out_tx.send(d).await.is_err() {
                                return;
                            }
                        }
                    });
                    return Ok(ShellSession {
                        input: in_tx,
                        output: out_rx,
                    });
                }
                let mut shell = MockShell::new(&req.name);
                tokio::spawn(async move {
                    if out_tx.send(shell.banner()).await.is_err() {
                        return;
                    }
                    while let Some(input) = in_rx.recv().await {
                        match input {
                            ShellInput::Data(d) => {
                                let (out, done) = shell.feed(&d);
                                if !out.is_empty() && out_tx.send(out).await.is_err() {
                                    return;
                                }
                                if done {
                                    return;
                                }
                            }
                            ShellInput::Resize { .. } => {}
                            ShellInput::Hangup => return,
                        }
                    }
                });
                Ok(ShellSession {
                    input: in_tx,
                    output: out_rx,
                })
            }
        }
    }
}

// ---- Incus ------------------------------------------------------------------------------------

/// How to reach Incus's websocket endpoints with exactly the HTTP client's trust rules: the
/// pinned server certificate, our client certificate and the name the pinned certificate carries.
#[derive(Clone)]
pub(crate) struct WsTarget {
    pub connect_host: String,
    pub port: u16,
    /// Name the pinned certificate is verified against (and sent as SNI).
    pub server_name: String,
    pub tls: Arc<rustls::ClientConfig>,
}

impl std::fmt::Debug for WsTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsTarget")
            .field("connect_host", &self.connect_host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl WsTarget {
    pub(crate) fn build(
        client_cert_pem: &[u8],
        client_key_pem: &[u8],
        server_cert_pem: &[u8],
        connect_host: String,
        port: u16,
        server_name: String,
    ) -> Result<Self, AdapterError> {
        use rustls_pki_types::pem::PemObject;
        use rustls_pki_types::{CertificateDer, PrivateKeyDer};
        let invalid = |what: &str, e: &dyn std::fmt::Display| {
            AdapterError::InvalidConfig(format!("{what}: {e}"))
        };
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(
                CertificateDer::from_pem_slice(server_cert_pem)
                    .map_err(|e| invalid("server certificate", &e))?,
            )
            .map_err(|e| invalid("server certificate", &e))?;
        let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(client_cert_pem)
            .collect::<Result<_, _>>()
            .map_err(|e| invalid("client certificate", &e))?;
        let key =
            PrivateKeyDer::from_pem_slice(client_key_pem).map_err(|e| invalid("client key", &e))?;
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| invalid("TLS", &e))?
            .with_root_certificates(roots)
            .with_client_auth_cert(chain, key)
            .map_err(|e| invalid("client certificate/key", &e))?;
        Ok(Self {
            connect_host,
            port,
            server_name,
            tls: Arc::new(tls),
        })
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>;

fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(4 * MAX_CHUNK))
        .max_frame_size(Some(2 * MAX_CHUNK))
}

impl WsTarget {
    /// Opens a websocket to an Incus path such as `/1.0/operations/<id>/websocket?secret=<s>`.
    /// Errors never include the path: it carries a one-time secret.
    async fn connect(&self, path_and_query: &str) -> Result<Ws, AdapterError> {
        let fail = |m: String| AdapterError::Connectivity(m);
        let tcp = tokio::net::TcpStream::connect((self.connect_host.as_str(), self.port))
            .await
            .map_err(|e| fail(format!("websocket: {e}")))?;
        let name = rustls_pki_types::ServerName::try_from(self.server_name.clone())
            .map_err(|e| fail(format!("websocket: TLS name: {e}")))?;
        let tls = tokio_rustls::TlsConnector::from(self.tls.clone())
            .connect(name, tcp)
            .await
            .map_err(|e| fail(format!("websocket: TLS: {e}")))?;
        let host = if self.server_name.contains(':') {
            format!("[{}]", self.server_name)
        } else {
            self.server_name.clone()
        };
        let request = format!("wss://{host}:{}{path_and_query}", self.port)
            .into_client_request()
            .map_err(|_| fail("websocket: invalid address".into()))?;
        let (ws, _) = tokio_tungstenite::client_async_with_config(request, tls, Some(ws_config()))
            .await
            .map_err(|e| fail(format!("websocket: {}", ws_error(&e))))?;
        Ok(ws)
    }
}

/// A short reason that cannot contain the request (and with it the secret).
fn ws_error(e: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error as E;
    match e {
        E::Http(r) => format!("refused with HTTP {}", r.status()),
        E::Io(e) => e.to_string(),
        other => other.to_string(),
    }
}

impl IncusClient {
    /// Starts `req.command` in a running, Apelora-owned instance with a terminal and returns the
    /// channels of the session. Verified only against a contract fixture, not a real Incus.
    pub async fn open_shell(&self, req: &ShellRequest) -> Result<ShellSession, AdapterError> {
        req.check()?;
        let ws = self.ws_target()?;
        match self.instance(&req.project, &req.name).await? {
            None => {
                return Err(AdapterError::NotFound(format!(
                    "instance {}/{}",
                    req.project, req.name
                )));
            }
            Some(s) if !s.managed => {
                return Err(AdapterError::Api {
                    status: 409,
                    message: "the instance is not managed by Apelora".into(),
                });
            }
            Some(s) if s.status != "Running" => {
                return Err(AdapterError::Api {
                    status: 409,
                    message: format!("the instance is {}", s.status.to_lowercase()),
                });
            }
            Some(_) => {}
        }
        let mut body = json!({
            "command": req.command,
            "environment": req.env,
            "wait-for-websocket": true,
            "interactive": req.interactive,
            "record-output": false,
        });
        if req.interactive {
            body["width"] = json!(req.cols);
            body["height"] = json!(req.rows);
        }
        if let Some(uid) = req.uid {
            body["user"] = json!(uid);
        }
        if let Some(gid) = req.gid {
            body["group"] = json!(gid);
        }
        if let Some(cwd) = &req.cwd {
            body["cwd"] = json!(cwd);
        }
        let path = format!("/1.0/instances/{}/exec?project={}", req.name, req.project);
        let env = self.call::<Value>(Method::POST, &path, Some(&body)).await?;
        let op = env.metadata.unwrap_or(Value::Null);
        let id = op["id"].as_str().unwrap_or_default().to_owned();
        let fd = |k: &str| op["metadata"]["fds"][k].as_str().map(str::to_owned);
        // With a terminal one channel carries both directions; without one, Incus hands out
        // standard input, output and error separately.
        let names: &[&str] = if req.interactive {
            &["0", "control"]
        } else {
            &["0", "1", "2", "control"]
        };
        let Some(secrets) = names.iter().map(|k| fd(k)).collect::<Option<Vec<String>>>() else {
            return Err(AdapterError::Decode(
                "the exec response has no websocket channels".into(),
            ));
        };
        if !crate::client::is_operation_id(&id) {
            return Err(AdapterError::Decode("the exec operation has no id".into()));
        }
        // The secrets go into a URL; accept only what a secret looks like.
        let plain = |x: &str| {
            !x.is_empty()
                && x.len() <= 128
                && x.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        };
        if !secrets.iter().all(|x| plain(x)) {
            self.cancel_operation(&id).await;
            return Err(AdapterError::Decode(
                "the exec channels have unexpected secrets".into(),
            ));
        }
        let socket = |secret: &str| format!("/1.0/operations/{id}/websocket?secret={secret}");
        let connected = tokio::time::timeout(CONNECT_TIMEOUT, async {
            let mut sockets = Vec::with_capacity(secrets.len());
            for secret in &secrets {
                sockets.push(ws.connect(&socket(secret)).await?);
            }
            Ok::<_, AdapterError>(sockets)
        })
        .await;
        let mut sockets = match connected {
            Ok(Ok(all)) => all,
            Ok(Err(e)) => {
                self.cancel_operation(&id).await;
                return Err(e);
            }
            Err(_) => {
                self.cancel_operation(&id).await;
                return Err(AdapterError::Connectivity(
                    "websocket: timed out connecting to the session".into(),
                ));
            }
        };
        let control = sockets
            .pop()
            .ok_or_else(|| AdapterError::Decode("no control".into()))?;
        if req.interactive {
            let data = sockets
                .pop()
                .ok_or_else(|| AdapterError::Decode("no data".into()))?;
            return Ok(pump(data, control));
        }
        let mut it = sockets.into_iter();
        match (it.next(), it.next(), it.next()) {
            (Some(stdin), Some(stdout), Some(stderr)) => {
                Ok(pump_streams(stdin, stdout, stderr, control))
            }
            _ => Err(AdapterError::Decode("missing exec channels".into())),
        }
    }

    fn ws_target(&self) -> Result<&WsTarget, AdapterError> {
        self.ws.as_ref().ok_or_else(|| {
            AdapterError::UnsupportedCapability("interactive sessions need a TLS connection".into())
        })
    }

    /// Best effort: stop an exec operation nobody will ever attach to.
    async fn cancel_operation(&self, id: &str) {
        let _ = self
            .call::<Value>(Method::DELETE, &format!("/1.0/operations/{id}"), None)
            .await;
    }
}

/// Sends within [`SEND_TIMEOUT`]; false when the other side is gone or has stopped reading.
async fn send_within<S>(sink: &mut S, m: Message) -> bool
where
    S: futures_util::Sink<Message> + Unpin,
{
    matches!(
        tokio::time::timeout(SEND_TIMEOUT, sink.send(m)).await,
        Ok(Ok(()))
    )
}

/// Moves bytes between the channels and the two websockets until either side ends. Nothing here
/// waits forever on a peer that has stopped reading: a stalled side ends the session.
fn pump(data: Ws, control: Ws) -> ShellSession {
    let (in_tx, mut in_rx) = mpsc::channel::<ShellInput>(32);
    let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(64);
    tokio::spawn(async move {
        let (mut data_w, mut data_r) = data.split();
        let (mut ctl_w, mut ctl_r) = control.split();
        let mut control_open = true;
        'session: loop {
            tokio::select! {
                msg = data_r.next() => match msg {
                    Some(Ok(Message::Binary(b))) => {
                        if !matches!(tokio::time::timeout(SEND_TIMEOUT, out_tx.send(b.to_vec())).await, Ok(Ok(()))) { break 'session; }
                    }
                    Some(Ok(Message::Text(t))) => {
                        if !matches!(tokio::time::timeout(SEND_TIMEOUT, out_tx.send(t.as_bytes().to_vec())).await, Ok(Ok(()))) { break 'session; }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break 'session,
                    Some(Ok(_)) => {}
                },
                // The control channel only matters for what we send; drain whatever arrives, and
                // stop polling it once it has ended (it would otherwise be ready forever).
                c = ctl_r.next(), if control_open => {
                    if matches!(c, None | Some(Err(_))) { control_open = false; }
                }
                input = in_rx.recv() => match input {
                    Some(ShellInput::Data(d)) => {
                        for chunk in d.chunks(MAX_CHUNK) {
                            if !send_within(&mut data_w, Message::Binary(chunk.to_vec().into())).await { break 'session; }
                        }
                    }
                    Some(ShellInput::Resize { cols, rows }) => {
                        let m = json!({"command": "window-resize", "args": {"width": cols.to_string(), "height": rows.to_string()}});
                        if control_open { let _ = send_within(&mut ctl_w, Message::Text(m.to_string().into())).await; }
                    }
                    // Hung up on purpose, or the consumer went away: the process gets SIGHUP.
                    Some(ShellInput::Hangup) | None => {
                        let m = json!({"command": "signal", "signal": 1});
                        if control_open { let _ = send_within(&mut ctl_w, Message::Text(m.to_string().into())).await; }
                        break 'session;
                    }
                },
            }
        }
        let _ = tokio::time::timeout(SEND_TIMEOUT, data_w.close()).await;
        let _ = tokio::time::timeout(SEND_TIMEOUT, ctl_w.close()).await;
    });
    ShellSession {
        input: in_tx,
        output: out_rx,
    }
}

/// Like [`pump`], for a session without a terminal: input goes to standard input, standard
/// output comes back unchanged, standard error is read and dropped (it would otherwise stall the
/// process once its buffer filled). The session ends when standard output ends.
fn pump_streams(stdin: Ws, stdout: Ws, stderr: Ws, control: Ws) -> ShellSession {
    let (in_tx, mut in_rx) = mpsc::channel::<ShellInput>(32);
    let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(64);
    tokio::spawn(async move {
        let (mut in_w, mut in_r) = stdin.split();
        let (mut out_w, mut out_r) = stdout.split();
        let (mut err_w, mut err_r) = stderr.split();
        let (mut ctl_w, mut ctl_r) = control.split();
        let (mut err_open, mut ctl_open, mut in_open) = (true, true, true);
        'session: loop {
            tokio::select! {
                msg = out_r.next() => match msg {
                    Some(Ok(Message::Binary(b))) => {
                        if !matches!(tokio::time::timeout(SEND_TIMEOUT, out_tx.send(b.to_vec())).await, Ok(Ok(()))) { break 'session; }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break 'session,
                    Some(Ok(_)) => {}
                },
                e = err_r.next(), if err_open => {
                    if matches!(e, None | Some(Err(_)) | Some(Ok(Message::Close(_)))) { err_open = false; }
                }
                c = ctl_r.next(), if ctl_open => {
                    if matches!(c, None | Some(Err(_))) { ctl_open = false; }
                }
                // Incus never writes to the standard-input socket; drain its closing.
                i = in_r.next(), if in_open => {
                    if matches!(i, None | Some(Err(_)) | Some(Ok(Message::Close(_)))) { in_open = false; }
                }
                input = in_rx.recv() => match input {
                    Some(ShellInput::Data(d)) => {
                        for chunk in d.chunks(MAX_CHUNK) {
                            if !send_within(&mut in_w, Message::Binary(chunk.to_vec().into())).await { break 'session; }
                        }
                    }
                    Some(ShellInput::Resize { .. }) => {}
                    Some(ShellInput::Hangup) | None => {
                        let m = json!({"command": "signal", "signal": 15});
                        if ctl_open { let _ = send_within(&mut ctl_w, Message::Text(m.to_string().into())).await; }
                        break 'session;
                    }
                },
            }
        }
        for w in [&mut in_w, &mut out_w, &mut err_w, &mut ctl_w] {
            let _ = tokio::time::timeout(SEND_TIMEOUT, w.close()).await;
        }
    });
    ShellSession {
        input: in_tx,
        output: out_rx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_shell_runs_a_few_commands_and_ends_on_exit() {
        let mut sh = MockShell::new("web-01");
        assert!(String::from_utf8_lossy(&sh.banner()).contains("MOCK"));
        let (out, done) = sh.feed(b"hostname\r");
        assert!(!done);
        assert!(String::from_utf8_lossy(&out).contains("web-01"));
        let (out, _) = sh.feed(b"rm -rf /\r");
        assert!(String::from_utf8_lossy(&out).contains("command not found"));
        let (_, done) = sh.feed(b"exit\r");
        assert!(done);
        let mut sh = MockShell::new("web-01");
        assert!(
            sh.feed(&[0x04]).1,
            "Ctrl-D on an empty line ends the session"
        );
    }

    #[test]
    fn requests_with_bad_names_commands_or_sizes_are_refused() {
        let ok = ShellRequest {
            project: "apelora".into(),
            name: "web-01".into(),
            command: vec!["/bin/sh".into()],
            uid: Some(1000),
            gid: Some(1000),
            cwd: Some("/tmp".into()),
            env: BTreeMap::new(),
            cols: 80,
            rows: 24,
            interactive: true,
        };
        assert!(ok.check().is_ok());
        let relay = ShellRequest {
            cols: 0,
            rows: 0,
            interactive: false,
            ..ok.clone()
        };
        assert!(
            relay.check().is_ok(),
            "the size means nothing without a terminal"
        );
        for bad in [
            ShellRequest {
                name: "../x".into(),
                ..ok.clone()
            },
            ShellRequest {
                project: String::new(),
                ..ok.clone()
            },
            ShellRequest {
                command: vec![],
                ..ok.clone()
            },
            ShellRequest {
                cols: 0,
                ..ok.clone()
            },
            ShellRequest {
                rows: 5000,
                ..ok.clone()
            },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
    }
}
