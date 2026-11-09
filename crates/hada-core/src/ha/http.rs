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
    pub body: String,
}

impl Response {
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.body).ok()
    }
}

/// `POST {base}{path}` with the given content type and body.
pub async fn post(
    base: &str,
    path: &str,
    content_type: &str,
    body: &str,
) -> Result<Response, HttpError> {
    let url = url::Url::parse(base.trim()).map_err(|e| HttpError::Url(e.to_string()))?;
    let host = url
        .host_str()
        .ok_or_else(|| HttpError::Url("missing host".into()))?
        .to_string();
    let tls = match url.scheme() {
        "http" | "ws" => false,
        "https" | "wss" => true,
        s => return Err(HttpError::Url(format!("unsupported scheme {s}"))),
    };
    let port = url.port().unwrap_or(if tls { 443 } else { 80 });
    let prefix = url.path().trim_end_matches('/');
    let request = format!(
        "POST {prefix}{path} HTTP/1.1\r\nHost: {host}:{port}\r\nUser-Agent: hada\r\nAccept: application/json\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let fut = async {
        let tcp = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(|e| HttpError::Connect(e.to_string()))?;
        if tls {
            let config = rustls_config();
            let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
            let name = rustls_pki_types::ServerName::try_from(host.clone())
                .map_err(|e| HttpError::Url(e.to_string()))?;
            let mut stream = connector
                .connect(name, tcp)
                .await
                .map_err(|e| HttpError::Connect(e.to_string()))?;
            stream.write_all(request.as_bytes()).await?;
            let mut raw = Vec::new();
            stream.read_to_end(&mut raw).await?;
            parse(&raw)
        } else {
            let mut stream = tcp;
            stream.write_all(request.as_bytes()).await?;
            let mut raw = Vec::new();
            stream.read_to_end(&mut raw).await?;
            parse(&raw)
        }
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
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "transfer-encoding" if v.trim().eq_ignore_ascii_case("chunked") => chunked = true,
                "content-length" => length = v.trim().parse().ok(),
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
