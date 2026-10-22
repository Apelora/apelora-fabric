// SPDX-License-Identifier: AGPL-3.0-only
//! IPMI over the LAN (IPMI 2.0, RMCP+ on UDP) for the BMC fence provider, limited to what fencing
//! needs: read the system GUID, read whether the chassis power is on, and switch it off hard.
//! Nothing else is sent (no serial console, media, users, sensors, boot devices or firmware).
//!
//! Every session is authenticated, integrity-protected and encrypted: cipher suite 17
//! (RAKP-HMAC-SHA256, HMAC-SHA256-128, AES-CBC-128) is proposed first and suite 3 (the same with
//! SHA-1) only when the BMC refuses 17. A BMC that answers with any other algorithm, including
//! "none" (suite 0), is refused. The session asks for the operator role, the least that may
//! switch the power. The password is the HMAC key; it is never sent, logged or put in an error.

use std::net::SocketAddr;
use std::time::Duration;

use aws_lc_rs::cipher::{
    AES_128, DecryptingKey, DecryptionContext, EncryptingKey, EncryptionContext, UnboundCipherKey,
};
use aws_lc_rs::hmac;
use aws_lc_rs::iv::FixedLength;
use tokio::net::UdpSocket;

/// How long one datagram is waited for, and how often a request is sent before giving up.
const WAIT: Duration = Duration::from_secs(2);
const SENDS: usize = 3;
/// The longest user name IPMI carries.
pub(super) const MAX_USER: usize = 16;
/// The operator role with a name-only lookup.
const ROLE: u8 = 0x10 | 0x03;
const OPERATOR: u8 = 0x03;

const RMCP: [u8; 4] = [0x06, 0x00, 0xff, 0x07];
const AUTH_RMCP_PLUS: u8 = 0x06;
const ENCRYPTED: u8 = 0x80;
const AUTHENTICATED: u8 = 0x40;

const PAYLOAD_IPMI: u8 = 0x00;
const OPEN_REQUEST: u8 = 0x10;
const OPEN_RESPONSE: u8 = 0x11;
const RAKP1: u8 = 0x12;
const RAKP2: u8 = 0x13;
const RAKP3: u8 = 0x14;
const RAKP4: u8 = 0x15;

const BMC_ADDR: u8 = 0x20;
const CONSOLE_ADDR: u8 = 0x81;
const NETFN_CHASSIS: u8 = 0x00;
const NETFN_APP: u8 = 0x06;

/// The cipher suites Apelora accepts, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Suite {
    /// RAKP-HMAC-SHA256, HMAC-SHA256-128, AES-CBC-128.
    S17,
    /// RAKP-HMAC-SHA1, HMAC-SHA1-96, AES-CBC-128.
    S3,
}

impl Suite {
    pub(super) fn number(self) -> u8 {
        match self {
            Suite::S17 => 17,
            Suite::S3 => 3,
        }
    }
    /// (authentication, integrity, confidentiality) algorithm numbers.
    pub(super) fn algorithms(self) -> [u8; 3] {
        match self {
            Suite::S17 => [0x03, 0x04, 0x01],
            Suite::S3 => [0x01, 0x01, 0x01],
        }
    }
    fn of(algorithms: [u8; 3]) -> Option<Suite> {
        [Suite::S17, Suite::S3]
            .into_iter()
            .find(|s| s.algorithms() == algorithms)
    }
    pub(super) fn mac(self, key: &[u8], data: &[u8]) -> Vec<u8> {
        let algorithm = match self {
            Suite::S17 => hmac::HMAC_SHA256,
            Suite::S3 => hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY,
        };
        hmac::sign(&hmac::Key::new(algorithm, key), data)
            .as_ref()
            .to_vec()
    }
    /// The length of a session message's integrity code and of RAKP 4's check value.
    pub(super) fn icv_len(self) -> usize {
        match self {
            Suite::S17 => 16,
            Suite::S3 => 12,
        }
    }
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut b = [0u8; N];
    aws_lc_rs::rand::fill(&mut b).map_err(|_| "no random numbers available".to_owned())?;
    Ok(b)
}

fn checksum(bytes: &[u8]) -> u8 {
    0u8.wrapping_sub(bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b)))
}

/// The keys a session derives from the exchange (IPMI 2.0, RAKP).
#[derive(Clone)]
pub(super) struct Keys {
    pub(super) sik: Vec<u8>,
    pub(super) k1: Vec<u8>,
    pub(super) aes: [u8; 16],
}

/// Everything both sides put into the RAKP codes.
pub(super) struct Exchange<'a> {
    pub(super) suite: Suite,
    pub(super) console_sid: [u8; 4],
    pub(super) bmc_sid: [u8; 4],
    pub(super) rm: [u8; 16],
    pub(super) rc: [u8; 16],
    pub(super) guid: [u8; 16],
    pub(super) role: u8,
    pub(super) user: &'a [u8],
    pub(super) password: &'a [u8],
}

impl Exchange<'_> {
    fn role_and_name(&self) -> Vec<u8> {
        let mut v = vec![self.role, self.user.len() as u8];
        v.extend_from_slice(self.user);
        v
    }
    /// The code in RAKP 2: the BMC shows it knows the password.
    pub(super) fn rakp2_code(&self) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&self.console_sid);
        d.extend_from_slice(&self.bmc_sid);
        d.extend_from_slice(&self.rm);
        d.extend_from_slice(&self.rc);
        d.extend_from_slice(&self.guid);
        d.extend(self.role_and_name());
        self.suite.mac(self.password, &d)
    }
    /// The code in RAKP 3: the console shows it knows the password.
    pub(super) fn rakp3_code(&self) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&self.rc);
        d.extend_from_slice(&self.console_sid);
        d.extend(self.role_and_name());
        self.suite.mac(self.password, &d)
    }
    /// Session keys. Without a separate BMC key the password is the key-generating key.
    pub(super) fn keys(&self) -> Keys {
        let mut d = Vec::new();
        d.extend_from_slice(&self.rm);
        d.extend_from_slice(&self.rc);
        d.extend(self.role_and_name());
        let sik = self.suite.mac(self.password, &d);
        let k1 = self.suite.mac(&sik, &[0x01; 20]);
        let k2 = self.suite.mac(&sik, &[0x02; 20]);
        let mut aes = [0u8; 16];
        aes.copy_from_slice(&k2[..16]);
        Keys { sik, k1, aes }
    }
    /// The check value in RAKP 4: the BMC derived the same session key.
    pub(super) fn rakp4_icv(&self, keys: &Keys) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&self.rm);
        d.extend_from_slice(&self.bmc_sid);
        d.extend_from_slice(&self.guid);
        let mut v = self.suite.mac(&keys.sik, &d);
        v.truncate(self.suite.icv_len());
        v
    }
}

/// One RMCP+ packet before a session (no integrity code, no encryption).
pub(super) fn plain_packet(payload_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut p = RMCP.to_vec();
    p.push(AUTH_RMCP_PLUS);
    p.push(payload_type);
    p.extend_from_slice(&[0; 8]);
    p.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    p.extend_from_slice(payload);
    p
}

/// Reads a packet before a session: its payload type and payload.
pub(super) fn parse_plain(p: &[u8]) -> Option<(u8, &[u8])> {
    if p.len() < 16 || p[..4] != RMCP || p[4] != AUTH_RMCP_PLUS {
        return None;
    }
    let len = u16::from_le_bytes([p[14], p[15]]) as usize;
    let body = p.get(16..16 + len)?;
    Some((p[5] & 0x3f, body))
}

/// One encrypted, integrity-protected packet in a session.
pub(super) fn session_packet(
    suite: Suite,
    keys: &Keys,
    to_sid: [u8; 4],
    seq: u32,
    payload_type: u8,
    payload: &[u8],
) -> Result<Vec<u8>, String> {
    let iv: [u8; 16] = random()?;
    let mut data = payload.to_vec();
    let pad = (16 - (data.len() + 1) % 16) % 16;
    data.extend((1..=pad as u8).collect::<Vec<_>>());
    data.push(pad as u8);
    let key = UnboundCipherKey::new(&AES_128, &keys.aes).map_err(|_| "AES key")?;
    EncryptingKey::cbc(key)
        .map_err(|_| "AES key")?
        .less_safe_encrypt(&mut data, EncryptionContext::Iv128(FixedLength::from(iv)))
        .map_err(|_| "AES encryption failed")?;
    let mut body = iv.to_vec();
    body.extend(data);

    let mut s = vec![AUTH_RMCP_PLUS, ENCRYPTED | AUTHENTICATED | payload_type];
    s.extend_from_slice(&to_sid);
    s.extend_from_slice(&seq.to_le_bytes());
    s.extend_from_slice(&(body.len() as u16).to_le_bytes());
    s.extend(body);
    let integrity_pad = (4 - (s.len() + 2) % 4) % 4;
    s.extend(std::iter::repeat_n(0xff, integrity_pad));
    s.push(integrity_pad as u8);
    s.push(0x07);
    let mut code = suite.mac(&keys.k1, &s);
    code.truncate(suite.icv_len());
    s.extend(code);
    let mut p = RMCP.to_vec();
    p.extend(s);
    Ok(p)
}

/// Checks and opens a packet in a session: (session id, sequence number, payload type, payload).
/// A packet that is not both authenticated and encrypted, or whose code does not match, is `None`.
pub(super) fn open_session_packet(
    suite: Suite,
    keys: &Keys,
    p: &[u8],
) -> Option<([u8; 4], u32, u8, Vec<u8>)> {
    let n = suite.icv_len();
    if p.len() < 4 + 12 + n + 2 || p[..4] != RMCP {
        return None;
    }
    let s = &p[4..];
    if s[0] != AUTH_RMCP_PLUS || s[1] & (ENCRYPTED | AUTHENTICATED) != (ENCRYPTED | AUTHENTICATED) {
        return None;
    }
    let (covered, code) = s.split_at(s.len() - n);
    let mut want = suite.mac(&keys.k1, covered);
    want.truncate(n);
    if !same(code, &want) || covered[covered.len() - 1] != 0x07 {
        return None;
    }
    let sid = [s[2], s[3], s[4], s[5]];
    let seq = u32::from_le_bytes([s[6], s[7], s[8], s[9]]);
    let len = u16::from_le_bytes([s[10], s[11]]) as usize;
    let body = s.get(12..12 + len)?;
    if body.len() < 32 || body.len() % 16 != 0 {
        return None;
    }
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&body[..16]);
    let mut data = body[16..].to_vec();
    let key = UnboundCipherKey::new(&AES_128, &keys.aes).ok()?;
    let plain = DecryptingKey::cbc(key)
        .ok()?
        .decrypt(&mut data, DecryptionContext::Iv128(FixedLength::from(iv)))
        .ok()?
        .to_vec();
    let pad = *plain.last()? as usize;
    if pad + 1 > plain.len() {
        return None;
    }
    Some((
        sid,
        seq,
        s[1] & 0x3f,
        plain[..plain.len() - 1 - pad].to_vec(),
    ))
}

/// An IPMI request in LAN framing.
pub(super) fn request(netfn: u8, rq_seq: u8, cmd: u8, data: &[u8]) -> Vec<u8> {
    let mut m = vec![BMC_ADDR, netfn << 2];
    m.push(checksum(&m));
    let start = m.len();
    m.extend_from_slice(&[CONSOLE_ADDR, rq_seq << 2, cmd]);
    m.extend_from_slice(data);
    m.push(checksum(&m[start..]));
    m
}

/// The data of the answer to `request(netfn, rq_seq, cmd, …)`, after its completion code.
fn answer(netfn: u8, rq_seq: u8, cmd: u8, m: &[u8]) -> Option<Result<Vec<u8>, u8>> {
    if m.len() < 8
        || m[0] != CONSOLE_ADDR
        || m[1] >> 2 != netfn | 1
        || checksum(&m[..2]) != m[2]
        || m[3] != BMC_ADDR
        || m[4] >> 2 != rq_seq
        || m[5] != cmd
        || checksum(&m[3..m.len() - 1]) != m[m.len() - 1]
    {
        return None;
    }
    Some(match m[6] {
        0 => Ok(m[7..m.len() - 1].to_vec()),
        cc => Err(cc),
    })
}

/// A status code of the session setup, in words.
fn setup_status(code: u8) -> String {
    match code {
        0x04..=0x07 | 0x10 | 0x11 => "the BMC offers neither cipher suite 17 nor 3".into(),
        0x09..=0x0b => "the BMC does not let this user act as operator".into(),
        0x0d => "the BMC does not know this user".into(),
        0x0f => "the BMC did not accept the password's proof (wrong password?)".into(),
        0x01 | 0x0c => "the BMC has no free session".into(),
        c => format!("the BMC refused the session (status 0x{c:02x})"),
    }
}

fn completion(cc: u8) -> String {
    match cc {
        0xd4 => "the BMC says the session may not do this (insufficient privilege)".into(),
        0xc1 => "the BMC does not support this command".into(),
        0xd5 => "the BMC cannot do this right now".into(),
        c => format!("the BMC answered with completion code 0x{c:02x}"),
    }
}

/// An open session with one BMC.
pub(super) struct Session {
    sock: UdpSocket,
    suite: Suite,
    keys: Keys,
    console_sid: [u8; 4],
    bmc_sid: [u8; 4],
    seq: u32,
    rq_seq: u8,
}

/// Sends `packet` and returns the first datagram from the BMC that `accept` takes, resending a
/// few times. Datagrams that do not parse or verify are ignored.
async fn exchange<T>(
    sock: &UdpSocket,
    mut packet: impl FnMut() -> Result<Vec<u8>, String>,
    mut accept: impl FnMut(&[u8]) -> Option<T>,
) -> Result<T, String> {
    let mut buf = vec![0u8; 1024];
    for _ in 0..SENDS {
        sock.send(&packet()?)
            .await
            .map_err(|e| format!("cannot send to the BMC: {e}"))?;
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            match tokio::time::timeout_at(deadline, sock.recv(&mut buf)).await {
                Err(_) => break,
                Ok(Err(e)) => return Err(format!("cannot read from the BMC: {e}")),
                Ok(Ok(n)) => {
                    if let Some(v) = accept(&buf[..n]) {
                        return Ok(v);
                    }
                }
            }
        }
    }
    Err(format!(
        "the BMC did not answer ({SENDS} tries, {} s each)",
        WAIT.as_secs()
    ))
}

/// Opens a session as `user` with the operator role. The password is used as the HMAC key only.
pub(super) async fn connect(
    addr: SocketAddr,
    user: &str,
    password: &str,
) -> Result<Session, String> {
    let local: SocketAddr = if addr.is_ipv4() {
        "0.0.0.0:0".parse().expect("address")
    } else {
        "[::]:0".parse().expect("address")
    };
    let sock = UdpSocket::bind(local)
        .await
        .map_err(|e| format!("cannot open a UDP socket: {e}"))?;
    sock.connect(addr)
        .await
        .map_err(|e| format!("cannot reach the BMC: {e}"))?;
    let mut refused = None;
    for suite in [Suite::S17, Suite::S3] {
        match open(&sock, suite, user.as_bytes(), password.as_bytes()).await? {
            Ok((keys, console_sid, bmc_sid, suite)) => {
                let mut s = Session {
                    sock,
                    suite,
                    keys,
                    console_sid,
                    bmc_sid,
                    seq: 0,
                    rq_seq: 0,
                };
                let level = s.call(NETFN_APP, 0x3b, &[OPERATOR]).await?;
                if level.first().map(|l| l & 0x0f) != Some(OPERATOR) {
                    return Err("the BMC did not grant the operator role".into());
                }
                return Ok(s);
            }
            Err(why) => refused = Some(why),
        }
    }
    Err(refused.unwrap_or_else(|| "the BMC refused the session".into()))
}

/// Proposes one cipher suite. `Ok(Err(_))`: the BMC refused this suite (try the next one).
async fn open(
    sock: &UdpSocket,
    suite: Suite,
    user: &[u8],
    password: &[u8],
) -> Result<Result<(Keys, [u8; 4], [u8; 4], Suite), String>, String> {
    let console_sid: [u8; 4] = random()?;
    let tag = random::<1>()?[0];
    let [auth, integrity, conf] = suite.algorithms();
    let mut req = vec![tag, OPERATOR, 0, 0];
    req.extend_from_slice(&console_sid);
    req.extend_from_slice(&[0x00, 0, 0, 0x08, auth, 0, 0, 0]);
    req.extend_from_slice(&[0x01, 0, 0, 0x08, integrity, 0, 0, 0]);
    req.extend_from_slice(&[0x02, 0, 0, 0x08, conf, 0, 0, 0]);
    let packet = plain_packet(OPEN_REQUEST, &req);
    let reply = exchange(
        sock,
        || Ok(packet.clone()),
        |p| {
            let (t, b) = parse_plain(p)?;
            (t == OPEN_RESPONSE && b.len() >= 2 && b[0] == tag).then(|| b.to_vec())
        },
    )
    .await?;
    if reply[1] != 0 {
        return Ok(Err(setup_status(reply[1])));
    }
    if reply.len() < 36 || reply[4..8] != console_sid {
        return Err("the BMC's answer to the session request is malformed".into());
    }
    let offered = [reply[16], reply[24], reply[32]];
    let Some(granted) = Suite::of(offered) else {
        return Err(format!(
            "the BMC offers algorithms {offered:?} (authentication, integrity, confidentiality): only cipher suites 17 and 3 are accepted, never one without authentication or encryption"
        ));
    };
    let mut bmc_sid = [0u8; 4];
    bmc_sid.copy_from_slice(&reply[8..12]);

    let rm: [u8; 16] = random()?;
    let mut r1 = vec![tag, 0, 0, 0];
    r1.extend_from_slice(&bmc_sid);
    r1.extend_from_slice(&rm);
    r1.extend_from_slice(&[ROLE, 0, 0, user.len() as u8]);
    r1.extend_from_slice(user);
    let packet = plain_packet(RAKP1, &r1);
    let r2 = exchange(
        sock,
        || Ok(packet.clone()),
        |p| {
            let (t, b) = parse_plain(p)?;
            (t == RAKP2 && b.len() >= 2 && b[0] == tag).then(|| b.to_vec())
        },
    )
    .await?;
    if r2[1] != 0 {
        return Err(setup_status(r2[1]));
    }
    let code_len = granted.mac(b"", b"").len();
    if r2.len() != 40 + code_len || r2[4..8] != console_sid {
        return Err("the BMC's RAKP 2 is malformed".into());
    }
    let mut x = Exchange {
        suite: granted,
        console_sid,
        bmc_sid,
        rm,
        rc: [0; 16],
        guid: [0; 16],
        role: ROLE,
        user,
        password,
    };
    x.rc.copy_from_slice(&r2[8..24]);
    x.guid.copy_from_slice(&r2[24..40]);
    if !same(&r2[40..], &x.rakp2_code()) {
        return Err(
            "the BMC's proof does not match the password: the password is wrong, or this is not the BMC the mapping names".into(),
        );
    }
    let keys = x.keys();
    let mut r3 = vec![tag, 0, 0, 0];
    r3.extend_from_slice(&bmc_sid);
    r3.extend(x.rakp3_code());
    let packet = plain_packet(RAKP3, &r3);
    let r4 = exchange(
        sock,
        || Ok(packet.clone()),
        |p| {
            let (t, b) = parse_plain(p)?;
            (t == RAKP4 && b.len() >= 2 && b[0] == tag).then(|| b.to_vec())
        },
    )
    .await?;
    if r4[1] != 0 {
        return Err(setup_status(r4[1]));
    }
    if r4.len() != 8 + granted.icv_len()
        || r4[4..8] != console_sid
        || !same(&r4[8..], &x.rakp4_icv(&keys))
    {
        return Err("the BMC's RAKP 4 does not check out: the session is not trusted".into());
    }
    Ok(Ok((keys, console_sid, bmc_sid, granted)))
}

impl Session {
    pub(super) fn suite(&self) -> Suite {
        self.suite
    }

    async fn call(&mut self, netfn: u8, cmd: u8, data: &[u8]) -> Result<Vec<u8>, String> {
        self.rq_seq = (self.rq_seq + 1) & 0x3f;
        let rq_seq = self.rq_seq;
        let message = request(netfn, rq_seq, cmd, data);
        let (suite, keys, bmc_sid, console_sid) = (
            self.suite,
            self.keys.clone(),
            self.bmc_sid,
            self.console_sid,
        );
        let seq = &mut self.seq;
        let got = exchange(
            &self.sock,
            || {
                *seq = seq.wrapping_add(1).max(1);
                session_packet(suite, &keys, bmc_sid, *seq, PAYLOAD_IPMI, &message)
            },
            |p| {
                let (sid, _, t, m) = open_session_packet(suite, &keys, p)?;
                if sid != console_sid || t != PAYLOAD_IPMI {
                    return None;
                }
                answer(netfn, rq_seq, cmd, &m)
            },
        )
        .await?;
        got.map_err(completion)
    }

    /// The system GUID as the BMC reports it (16 bytes, in the BMC's byte order).
    pub(super) async fn guid(&mut self) -> Result<[u8; 16], String> {
        let d = self.call(NETFN_APP, 0x37, &[]).await?;
        <[u8; 16]>::try_from(d.as_slice())
            .map_err(|_| format!("the BMC's system GUID has {} bytes, not 16", d.len()))
    }

    /// Whether the chassis power is on.
    pub(super) async fn power_on(&mut self) -> Result<bool, String> {
        let d = self.call(NETFN_CHASSIS, 0x01, &[]).await?;
        d.first()
            .map(|b| b & 0x01 == 0x01)
            .ok_or_else(|| "the BMC's chassis status is empty".into())
    }

    /// Switches the chassis power off at once (no orderly shutdown).
    pub(super) async fn power_off(&mut self) -> Result<(), String> {
        self.call(NETFN_CHASSIS, 0x02, &[0x00]).await.map(|_| ())
    }

    /// Ends the session; a BMC that does not answer is left to time it out.
    pub(super) async fn close(mut self) {
        let sid = self.bmc_sid;
        let _ = tokio::time::timeout(WAIT, self.call(NETFN_APP, 0x3c, &sid)).await;
    }
}

/// Whether the GUID a BMC reports is the mapped UUID. BMCs differ in byte order: as written,
/// fully reversed, or with the first three fields little-endian (as SMBIOS stores it).
pub(super) fn guid_matches(reported: &[u8; 16], mapped: &str) -> bool {
    let Ok(u) = uuid::Uuid::parse_str(mapped) else {
        return false;
    };
    let written = *u.as_bytes();
    let mut reversed = written;
    reversed.reverse();
    let mixed = u.to_bytes_le();
    [written, reversed, mixed].iter().any(|g| g == reported)
}

/// The GUID as text, for an error message.
pub(super) fn guid_text(g: &[u8; 16]) -> String {
    uuid::Uuid::from_bytes(*g).to_string()
}

#[cfg(test)]
mod tests;
