//! Tiny HTTP/1.1 client for the handful of REST calls we need (the OAuth-style
//! auth endpoints). Plain `http://` over TCP and `https://` through rustls,
//! reusing the TLS stack that the websocket already links.

use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("invalid URL: {0}")]
    Url(String),
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("request timed out")]
    Timeout,
    #[error("malformed HTTP response")]
    Malformed,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    /// The body as text (lossy UTF-8).
    pub body: String,
    /// The body as received (images).
    pub bytes: Vec<u8>,
    /// `Location` header of a redirect.
    pub location: Option<String>,
}

impl Response {
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.body).ok()
    }
}

/// Largest response read (album art is far smaller).
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;

/// `POST {base}{path}` with the given content type and body.
pub async fn post(
    base: &str,
    path: &str,
    content_type: &str,
    body: &str,
) -> Result<Response, HttpError> {
    let url = url::Url::parse(base.trim()).map_err(|e| HttpError::Url(e.to_string()))?;
    let (host, port, _) = endpoint(&url)?;
    let prefix = url.path().trim_end_matches('/');
    let request = format!(
        "POST {prefix}{path} HTTP/1.1\r\nHost: {host}:{port}\r\nUser-Agent: hada\r\nAccept: application/json\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    send(&url, request.as_bytes()).await
}

/// `GET url`, following up to three redirects. For images: see [`Response::bytes`].
pub async fn get(url: &str) -> Result<Response, HttpError> {
    let mut url = url::Url::parse(url.trim()).map_err(|e| HttpError::Url(e.to_string()))?;
    for _ in 0..=MAX_REDIRECTS {
        let (host, port, _) = endpoint(&url)?;
        let target = match url.query() {
            Some(q) => format!("{}?{q}", url.path()),
            None => url.path().to_string(),
        };
        let request = format!(
            "GET {target} HTTP/1.1\r\nHost: {host}:{port}\r\nUser-Agent: hada\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        );
        let resp = send(&url, request.as_bytes()).await?;
        match (resp.status, &resp.location) {
            (301 | 302 | 303 | 307 | 308, Some(loc)) => {
                url = url.join(loc).map_err(|e| HttpError::Url(e.to_string()))?;
            }
            _ => return Ok(resp),
        }
    }
    Err(HttpError::Url("too many redirects".into()))
}

/// Host, port and TLS for an http(s)/ws(s) URL.
fn endpoint(url: &url::Url) -> Result<(String, u16, bool), HttpError> {
    let host = url
        .host_str()
        .ok_or_else(|| HttpError::Url("missing host".into()))?
        .to_string();
    let tls = match url.scheme() {
        "http" | "ws" => false,
        "https" | "wss" => true,
        s => return Err(HttpError::Url(format!("unsupported scheme {s}"))),
    };
    Ok((host, url.port().unwrap_or(if tls { 443 } else { 80 }), tls))
}

async fn send(url: &url::Url, request: &[u8]) -> Result<Response, HttpError> {
    let (host, port, tls) = endpoint(url)?;
    let fut = async {
        let tcp = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(|e| HttpError::Connect(e.to_string()))?;
        let mut raw = Vec::new();
        if tls {
            let config = rustls_config();
            let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
            let name = rustls_pki_types::ServerName::try_from(host.clone())
                .map_err(|e| HttpError::Url(e.to_string()))?;
            let mut stream = connector
                .connect(name, tcp)
                .await
                .map_err(|e| HttpError::Connect(e.to_string()))?;
            stream.write_all(request).await?;
            stream.take(MAX_RESPONSE).read_to_end(&mut raw).await?;
        } else {
            let mut stream = tcp;
            stream.write_all(request).await?;
            stream.take(MAX_RESPONSE).read_to_end(&mut raw).await?;
        }
        parse(&raw)
    };
    tokio::time::timeout(TIMEOUT, fut)
        .await
        .map_err(|_| HttpError::Timeout)?
}

fn rustls_config() -> tokio_rustls::rustls::ClientConfig {
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// Parse a complete HTTP/1.1 response (Content-Length or chunked body).
pub fn parse(raw: &[u8]) -> Result<Response, HttpError> {
    let split = find(raw, b"\r\n\r\n").ok_or(HttpError::Malformed)?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| HttpError::Malformed)?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or(HttpError::Malformed)?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or(HttpError::Malformed)?;
    let mut chunked = false;
    let mut length: Option<usize> = None;
    let mut location = None;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "transfer-encoding" if v.trim().eq_ignore_ascii_case("chunked") => chunked = true,
                "content-length" => length = v.trim().parse().ok(),
                "location" => location = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    let body_raw = &raw[split + 4..];
    let body = if chunked {
        dechunk(body_raw)?
    } else {
        let n = length.unwrap_or(body_raw.len()).min(body_raw.len());
        body_raw[..n].to_vec()
    };
    Ok(Response {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
        bytes: body,
        location,
    })
}

fn dechunk(mut data: &[u8]) -> Result<Vec<u8>, HttpError> {
    let mut out = Vec::new();
    loop {
        let eol = find(data, b"\r\n").ok_or(HttpError::Malformed)?;
        let size_str = std::str::from_utf8(&data[..eol]).map_err(|_| HttpError::Malformed)?;
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("0").trim(), 16)
            .map_err(|_| HttpError::Malformed)?;
        data = &data[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size {
            return Err(HttpError::Malformed);
        }
        out.extend_from_slice(&data[..size]);
        data = data.get(size + 2..).ok_or(HttpError::Malformed)?;
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `application/x-www-form-urlencoded` body.
pub fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_length_and_chunked() {
        let r = parse(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
        )
        .unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "{}");
        let r = parse(b"HTTP/1.1 400 Bad Request\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n").unwrap();
        assert_eq!(r.status, 400);
        assert_eq!(r.body, "abcde");
        assert!(parse(b"garbage").is_err());
        let r = parse(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n\x89PN\xff").unwrap();
        assert_eq!(r.bytes, b"\x89PN\xff", "binary bodies are kept as they are");
        let r = parse(b"HTTP/1.1 302 Found\r\nLocation: /cover.jpg\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
        assert_eq!(r.location.as_deref(), Some("/cover.jpg"));
    }

    #[tokio::test]
    async fn get_follows_redirects() {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            for reply in [
                &b"HTTP/1.1 302 Found\r\nLocation: /art.png?x=1\r\nContent-Length: 0\r\n\r\n"[..],
                &b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n\x00\x01\x02"[..],
            ] {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 1024];
                let n = s.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                assert!(req.starts_with("GET /"), "{req}");
                s.write_all(reply).await.unwrap();
            }
        });
        let r = get(&format!("{base}/start")).await.unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.bytes, vec![0, 1, 2]);
    }

    #[test]
    fn encodes_forms() {
        assert_eq!(
            form_encode(&[
                ("grant_type", "authorization_code"),
                ("client_id", "http://localhost:8123/")
            ]),
            "grant_type=authorization_code&client_id=http%3A%2F%2Flocalhost%3A8123%2F"
        );
    }
}
