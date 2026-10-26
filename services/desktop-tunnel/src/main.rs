// SPDX-License-Identifier: AGPL-3.0-only
//! `apelora-desktop-tunnel`: lets a person's own remote desktop client reach a Windows module
//! through the Apelora control plane, without the module's RDP port being published anywhere.
//!
//! The person asks for a remote desktop session in the Fabric Console, which answers with a
//! single-use ticket valid for about a minute. This program reads the ticket from standard input
//! (never from the command line, which shells keep in their history), listens on a loopback
//! port, and when the remote desktop client connects there, opens the session's websocket with
//! the ticket and carries the bytes both ways. The control plane relays them to the guest's own
//! RDP port; TLS and network level authentication run end to end between the client and Windows,
//! so neither this program nor the control plane sees the password.
//!
//! One connection, one session: the program ends when either side does.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// The control plane's session protocol (`sessions::PROTOCOL`).
const PROTOCOL: &str = "apelora.session.v1";
/// A free port chosen by the system, so another program on this computer cannot wait on a
/// known one; the tunnel prints the port it got.
const DEFAULT_LISTEN: &str = "127.0.0.1:0";
/// The control plane accepts at most 16 KiB per message.
const CHUNK: usize = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

const USAGE: &str =
    "usage: apelora-desktop-tunnel --url wss://<apelora host>/api/v1/sessions/connect \
[--origin https://<console host>] [--listen 127.0.0.1:<port>] [--ca <PEM file>]

The Fabric Console shows the exact command for a session. The ticket is read from standard input.";

#[derive(Debug, PartialEq, Eq)]
struct Args {
    url: String,
    /// The page the ticket was issued to; the control plane refuses the ticket from any other.
    origin: Option<String>,
    listen: SocketAddr,
    /// A certificate authority to trust instead of the operating system's (an internal one).
    ca: Option<PathBuf>,
}

/// `host[:port]` of `scheme://host[:port]/…`, or `None`.
fn authority(url: &str, scheme: &str) -> Option<String> {
    let rest = url.strip_prefix(scheme)?;
    let end = rest.find('/').unwrap_or(rest.len());
    let a = &rest[..end];
    (!a.is_empty()).then(|| a.to_ascii_lowercase())
}

/// A literal loopback address (`127.0.0.0/8`, `::1`). Names are not trusted: what `localhost`
/// resolves to is up to the computer's configuration.
fn loopback(host: &str) -> bool {
    host.trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

fn parse(args: &[String]) -> Result<Args, String> {
    let (mut url, mut origin, mut listen, mut ca) = (None, None, None, None);
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        if matches!(flag, "-h" | "--help") {
            return Err(USAGE.into());
        }
        let value = args
            .get(i + 1)
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value\n\n{USAGE}"))?;
        match flag {
            "--url" => url = Some(value),
            "--origin" => origin = Some(value.trim_end_matches('/').to_owned()),
            "--listen" => listen = Some(value),
            "--ca" => ca = Some(PathBuf::from(value)),
            other => return Err(format!("unknown argument {other:?}\n\n{USAGE}")),
        }
        i += 2;
    }
    let url = url.ok_or_else(|| format!("--url is required\n\n{USAGE}"))?;
    if url.contains(['?', '#']) {
        return Err("--url must not carry a query: the ticket never goes into an address".into());
    }
    // Judge the host the connection will go to, as the websocket library reads the address.
    let host = url
        .as_str()
        .into_client_request()
        .ok()
        .and_then(|r| r.uri().host().map(str::to_owned))
        .ok_or_else(|| "--url is not a valid address".to_owned())?;
    let secure = authority(&url, "wss://");
    let plain = authority(&url, "ws://");
    if secure
        .as_deref()
        .or(plain.as_deref())
        .is_some_and(|a| a.contains('@'))
    {
        return Err("--url must not carry a user name or password".into());
    }
    match (secure, plain) {
        (Some(_), _) => {}
        (None, Some(_)) if loopback(&host) => {}
        (None, Some(_)) => {
            return Err(
                "--url must start with wss:// (plain ws:// only to this computer, for development)"
                    .into(),
            );
        }
        (None, None) => return Err("--url must start with wss://".into()),
    }
    if let Some(o) = &origin {
        let a = authority(o, "https://").or_else(|| authority(o, "http://"));
        if a.is_none() || o.matches('/').count() != 2 {
            return Err(format!(
                "--origin {o:?} is not an origin like https://console.example.test"
            ));
        }
    }
    let listen: SocketAddr = listen
        .as_deref()
        .unwrap_or(DEFAULT_LISTEN)
        .parse()
        .map_err(|_| "--listen must be an address and port, e.g. 127.0.0.1:13389".to_owned())?;
    // A name is never accepted here, only a literal address, so nothing resolves elsewhere.
    if !listen.ip().is_loopback() {
        return Err(
            "--listen must be a loopback address: the tunnel is for this computer only".into(),
        );
    }
    Ok(Args {
        url,
        origin,
        listen,
        ca,
    })
}

/// A ticket is what the control plane issues: 64 hexadecimal characters.
fn check_ticket(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(t.to_ascii_lowercase())
    } else {
        Err("that is not a session ticket: copy the 64-character ticket the Console shows".into())
    }
}

/// Plain words for what the control plane says on the socket (`closed` with a reason, `error`).
fn describe(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    match v["type"].as_str()? {
        "error" => v["message"].as_str().map(str::to_owned),
        "closed" => Some(
            match v["reason"].as_str().unwrap_or_default() {
                "exited" => "the remote desktop service in the module closed the connection",
                "idle" => "the session ended after a period without input",
                "max-duration" => "the session reached its time limit; ask for a new one",
                "terminated" => {
                    "the session was ended by an administrator or from the sessions list"
                }
                "access-revoked" => "the session ended because your access changed",
                "access-unverifiable" => {
                    "the session ended because the control plane could not confirm your access"
                }
                "rate-limited" => "the session ended because input arrived too fast",
                "stalled" => "the session ended because the connection stopped accepting data",
                "client-closed" => "you ended the session",
                _ => "the session ended",
            }
            .to_owned(),
        ),
        _ => None,
    }
}

/// Why the control plane refused the websocket, from its JSON answer, without echoing anything
/// of the request (it carries the ticket).
fn refusal(e: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error as E;
    match e {
        E::Http(r) => {
            let message = r
                .body()
                .as_deref()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok())
                .and_then(|v| v["error"]["message"].as_str().map(str::to_owned));
            match message {
                Some(m) => format!("Apelora refused the session ({}): {m}", r.status()),
                None => format!("Apelora refused the session ({})", r.status()),
            }
        }
        E::Io(e) => format!("cannot reach Apelora: {e}"),
        other => format!("the session could not be opened: {other}"),
    }
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

fn tls_config(ca: Option<&PathBuf>) -> Result<rustls::ClientConfig, String> {
    use rustls_pki_types::CertificateDer;
    use rustls_pki_types::pem::PemObject;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS: {e}"))?;
    match ca {
        Some(path) => {
            let mut roots = rustls::RootCertStore::empty();
            for cert in CertificateDer::pem_file_iter(path)
                .map_err(|e| format!("--ca {}: {e}", path.display()))?
            {
                roots
                    .add(cert.map_err(|e| format!("--ca {}: {e}", path.display()))?)
                    .map_err(|e| format!("--ca {}: {e}", path.display()))?;
            }
            Ok(builder.with_root_certificates(roots).with_no_client_auth())
        }
        None => {
            use rustls_platform_verifier::BuilderVerifierExt;
            Ok(builder
                .with_platform_verifier()
                .map_err(|e| format!("TLS: the system's trust store: {e}"))?
                .with_no_client_auth())
        }
    }
}

/// Opens the session's websocket: the ticket rides along as a sub-protocol, as in the browser.
async fn open_socket(a: &Args, ticket: &str) -> Result<WebSocketStream<Box<dyn Io>>, String> {
    let mut req = a
        .url
        .as_str()
        .into_client_request()
        .map_err(|_| "--url is not a valid address".to_owned())?;
    let header = |v: String| {
        v.parse()
            .map_err(|_| "a header could not be built".to_owned())
    };
    req.headers_mut().insert(
        "sec-websocket-protocol",
        header(format!("{PROTOCOL}, ticket.{ticket}"))?,
    );
    if let Some(o) = &a.origin {
        req.headers_mut().insert("origin", header(o.clone())?);
    }
    let secure = a.url.starts_with("wss://");
    let host = req
        .uri()
        .host()
        .ok_or_else(|| "--url has no host".to_owned())?
        .trim_matches(['[', ']'])
        .to_owned();
    let port = req
        .uri()
        .port_u16()
        .unwrap_or(if secure { 443 } else { 80 });
    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| format!("cannot reach {host}:{port}: {e}"))?;
    let stream: Box<dyn Io> = if secure {
        let name = rustls_pki_types::ServerName::try_from(host.clone())
            .map_err(|_| format!("{host} is not a valid name for TLS"))?;
        let tls = tokio_rustls::TlsConnector::from(Arc::new(tls_config(a.ca.as_ref())?))
            .connect(name, tcp)
            .await
            .map_err(|e| format!("TLS with {host}: {e}"))?;
        Box::new(tls)
    } else {
        Box::new(tcp)
    };
    let (ws, _) = tokio_tungstenite::client_async(req, stream)
        .await
        .map_err(|e| refusal(&e))?;
    Ok(ws)
}

/// Carries bytes between the remote desktop client and the session until either side ends;
/// returns why it ended.
async fn relay<S>(client: TcpStream, ws: WebSocketStream<S>) -> String
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut from_client, mut to_client) = client.into_split();
    let (mut to_apelora, mut from_apelora) = ws.split();
    let mut buf = vec![0u8; CHUNK];
    let mut said: Option<String> = None;
    loop {
        tokio::select! {
            n = from_client.read(&mut buf) => match n {
                Ok(0) | Err(_) => {
                    let _ = to_apelora.close().await;
                    return "your remote desktop client disconnected".into();
                }
                Ok(n) => {
                    if to_apelora.send(Message::Binary(buf[..n].to_vec().into())).await.is_err() {
                        return said.unwrap_or_else(|| "the connection to Apelora broke".into());
                    }
                }
            },
            m = from_apelora.next() => match m {
                Some(Ok(Message::Binary(b))) => {
                    if to_client.write_all(&b).await.is_err() {
                        let _ = to_apelora.close().await;
                        return "your remote desktop client disconnected".into();
                    }
                }
                Some(Ok(Message::Text(t))) => {
                    if let Some(words) = describe(&t) {
                        said = Some(words);
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                    let _ = to_client.shutdown().await;
                    return said.unwrap_or_else(|| "the session ended".into());
                }
                Some(Ok(_)) => {}
            },
        }
    }
}

/// Waits for the one remote desktop client, then opens the session and relays it.
async fn serve(listener: TcpListener, a: &Args, ticket: &str) -> Result<String, String> {
    let (client, peer) = listener
        .accept()
        .await
        .map_err(|e| format!("waiting for the remote desktop client: {e}"))?;
    drop(listener);
    if !peer.ip().is_loopback() {
        return Err("a connection came from outside this computer; nothing was opened".into());
    }
    let ws = tokio::time::timeout(CONNECT_TIMEOUT, open_socket(a, ticket))
        .await
        .map_err(|_| "Apelora did not answer in time".to_owned())??;
    eprintln!("Connected. Sign in to Windows in your remote desktop client.");
    Ok(relay(client, ws).await)
}

async fn run(a: Args) -> Result<String, String> {
    // Listen first, so a port already in use shows before the ticket is spent.
    let listener = TcpListener::bind(a.listen)
        .await
        .map_err(|e| format!("cannot listen on {}: {e}", a.listen))?;
    let local = listener
        .local_addr()
        .map_err(|e| format!("cannot listen on {}: {e}", a.listen))?;
    eprint!("Paste the session ticket from the Fabric Console, then press Enter: ");
    let mut line = String::new();
    tokio::io::BufReader::new(tokio::io::stdin())
        .read_line(&mut line)
        .await
        .map_err(|e| format!("reading the ticket: {e}"))?;
    let ticket = check_ticket(&line)?;
    eprintln!(
        "Now connect your remote desktop client to {local} (within a minute: the ticket expires)."
    );
    serve(listener, &a, &ticket).await
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a = match parse(&args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("apelora-desktop-tunnel: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(a)) {
        Ok(why) => {
            eprintln!("Session over: {why}.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("apelora-desktop-tunnel: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::result_large_err)] // test fixtures; the handshake callback's type is the websocket library's
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

    fn args(list: &[&str]) -> Result<Args, String> {
        parse(&list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn only_secure_addresses_and_a_loopback_listener_are_accepted() {
        let a = args(&[
            "--url",
            "wss://apelora.example.test/api/v1/sessions/connect",
            "--origin",
            "https://console.example.test/",
        ])
        .unwrap();
        assert_eq!(a.listen, DEFAULT_LISTEN.parse().unwrap());
        assert_eq!(a.origin.as_deref(), Some("https://console.example.test"));
        for bad in [
            vec!["--url", "ws://apelora.example.test/api/v1/sessions/connect"],
            vec!["--url", "https://apelora.example.test/"],
            vec!["--url", "wss://apelora.example.test/connect?ticket=abc"],
            vec![
                "--url",
                "wss://apelora.example.test/c",
                "--listen",
                "0.0.0.0:13389",
            ],
            vec![
                "--url",
                "wss://apelora.example.test/c",
                "--origin",
                "https://x/y",
            ],
            // A user name before the host must not pass the loopback test for another host.
            vec![
                "--url",
                "ws://127.0.0.1:80@apelora.example.test/api/v1/sessions/connect",
            ],
            vec!["--url", "ws://[::1]:80@apelora.example.test/c"],
            vec!["--url", "wss://user:pw@apelora.example.test/c"],
            // Only literal loopback addresses get plain ws://.
            vec!["--url", "ws://localhost:8080/c"],
            vec!["--url"],
            vec!["--port", "1"],
            vec![],
        ] {
            assert!(args(&bad).is_err(), "{bad:?}");
        }
        assert!(args(&["--url", "ws://127.0.0.1:8080/api/v1/sessions/connect"]).is_ok());
        assert!(args(&["--url", "ws://[::1]:8080/c", "--listen", "[::1]:3390"]).is_ok());
    }

    #[test]
    fn a_ticket_is_sixty_four_hex_characters() {
        let t = "a".repeat(64);
        assert_eq!(check_ticket(&format!("  {t}\n")).unwrap(), t);
        assert!(check_ticket("abc").is_err());
        assert!(check_ticket(&"g".repeat(64)).is_err());
    }

    #[test]
    fn the_control_planes_messages_are_put_into_words() {
        assert_eq!(
            describe(r#"{"type":"closed","reason":"idle"}"#).as_deref(),
            Some("the session ended after a period without input")
        );
        assert_eq!(
            describe(r#"{"type":"error","message":"the instance is not running"}"#).as_deref(),
            Some("the instance is not running")
        );
        assert_eq!(describe("not json"), None);
    }

    /// A stand-in for the control plane: checks the protocol, the ticket and the origin, then
    /// echoes every byte back and finally says why the session closed.
    async fn fake_apelora(ticket: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let check = |req: &Request, mut resp: Response| -> Result<Response, ErrorResponse> {
                let protocols = req
                    .headers()
                    .get("sec-websocket-protocol")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default();
                let origin = req
                    .headers()
                    .get("origin")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default();
                if protocols == format!("{PROTOCOL}, ticket.{ticket}")
                    && origin == "https://console.example.test"
                    && !req.uri().to_string().contains(ticket)
                {
                    resp.headers_mut()
                        .insert("sec-websocket-protocol", PROTOCOL.parse().unwrap());
                    Ok(resp)
                } else {
                    // What the control plane answers for a spent or unknown ticket.
                    let mut e = ErrorResponse::new(Some(
                        r#"{"error":{"code":"ticket_invalid","message":"the session ticket is unknown, used or expired"}}"#.into(),
                    ));
                    *e.status_mut() =
                        tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED;
                    Err(e)
                }
            };
            let mut ws = tokio_tungstenite::accept_hdr_async(tcp, check)
                .await
                .unwrap();
            while let Some(Ok(m)) = ws.next().await {
                match m {
                    Message::Binary(b) if b.as_ref() == b"bye" => {
                        let _ = ws
                            .send(Message::Text(
                                r#"{"type":"closed","reason":"max-duration"}"#.into(),
                            ))
                            .await;
                        let _ = ws.close(None).await;
                        return;
                    }
                    Message::Binary(b) => {
                        let _ = ws.send(Message::Binary(b)).await;
                    }
                    _ => {}
                }
            }
        });
        addr
    }

    #[tokio::test]
    async fn the_client_s_bytes_go_through_the_session_unchanged_and_the_end_is_explained() {
        const TICKET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let apelora = fake_apelora(TICKET).await;
        let a = args(&[
            "--url",
            &format!("ws://{apelora}/api/v1/sessions/connect"),
            "--origin",
            "https://console.example.test",
            "--listen",
            "127.0.0.1:0",
        ])
        .unwrap();
        let listener = TcpListener::bind(a.listen).await.unwrap();
        let local = listener.local_addr().unwrap();
        let tunnel = tokio::spawn(async move { serve(listener, &a, TICKET).await });

        let mut client = TcpStream::connect(local).await.unwrap();
        let hello = [0x03u8, 0x00, 0x00, 0x13, 0x0e, 0xe0, 0xff, 0x80];
        client.write_all(&hello).await.unwrap();
        let mut back = [0u8; 8];
        tokio::time::timeout(Duration::from_secs(5), client.read_exact(&mut back))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(back, hello);
        client.write_all(b"bye").await.unwrap();
        let why = tokio::time::timeout(Duration::from_secs(5), tunnel)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(why, "the session reached its time limit; ask for a new one");
    }

    #[tokio::test]
    async fn a_refusal_is_reported_without_the_ticket() {
        const TICKET: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        // The fake expects another ticket, so it refuses this one.
        let apelora =
            fake_apelora("0000000000000000000000000000000000000000000000000000000000000000").await;
        let a = args(&[
            "--url",
            &format!("ws://{apelora}/api/v1/sessions/connect"),
            "--origin",
            "https://console.example.test",
            "--listen",
            "127.0.0.1:0",
        ])
        .unwrap();
        let listener = TcpListener::bind(a.listen).await.unwrap();
        let local = listener.local_addr().unwrap();
        let tunnel = tokio::spawn(async move { serve(listener, &a, TICKET).await });
        let _client = TcpStream::connect(local).await.unwrap();
        let err = tokio::time::timeout(Duration::from_secs(5), tunnel)
            .await
            .unwrap()
            .unwrap()
            .expect_err("refused");
        assert!(err.contains("refused the session (401"), "{err}");
        assert!(err.contains("unknown, used or expired"), "{err}");
        assert!(!err.contains(TICKET), "{err}");
    }
}
