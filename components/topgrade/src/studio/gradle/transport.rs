//! Bound each HTTP response before trusting or appending its bytes.
use color_eyre::eyre::{Context, Report, Result, ensure, eyre};
use std::{
    ffi::OsString,
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    os::unix::ffi::OsStringExt,
    path::Path,
    process::{Child, ChildStdout, ExitStatus, Stdio},
    thread,
    time::Duration,
};

use super::super::{sha256, tool_command};
use super::{API, Nightly};

const MAX_HEADERS: u64 = 64 * 1024;
const MAX_METADATA: u64 = 256 * 1024;
const MAX_ZIP: u64 = 1024 * 1024 * 1024;
const PROXIES: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

#[derive(Debug)]
pub(super) enum Failure {
    Retry(String),
    Fatal(Report),
}

impl From<Report> for Failure {
    fn from(error: Report) -> Self {
        Self::Fatal(error)
    }
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self::Fatal(error.into())
    }
}

type Transfer<T> = std::result::Result<T, Failure>;

#[derive(Default)]
struct Headers {
    status: u16,
    length: Option<u64>,
    etag: Option<Vec<u8>>,
    range: Option<String>,
    encoding: Option<String>,
    location: Option<String>,
}

fn line(reader: &mut impl BufRead, left: &mut u64) -> Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let size = reader.take(*left + 1).read_until(b'\n', &mut bytes)? as u64;
    if size == 0 {
        return Ok(None);
    }
    ensure!(size <= *left, "Gradle response headers exceeded the size limit");
    *left -= size;
    ensure!(bytes.ends_with(b"\r\n"), "Incomplete Gradle HTTP header");
    bytes.truncate(bytes.len() - 2);
    Ok(Some(bytes))
}

fn headers(reader: &mut impl BufRead) -> Result<Option<Headers>> {
    let mut left = MAX_HEADERS;
    for _ in 0..4 {
        let Some(status) = line(reader, &mut left)? else {
            return Ok(None);
        };
        let mut fields = std::str::from_utf8(&status)?.split_ascii_whitespace();
        ensure!(
            matches!(fields.next(), Some("HTTP/1.0" | "HTTP/1.1" | "HTTP/2" | "HTTP/3")),
            "Invalid Gradle HTTP status line"
        );
        let status = fields
            .next()
            .and_then(|field| field.parse::<u16>().ok())
            .filter(|status| (100..=599).contains(status))
            .ok_or_else(|| eyre!("Invalid Gradle HTTP status code"))?;
        let mut headers = Headers {
            status,
            ..Headers::default()
        };
        loop {
            let raw = line(reader, &mut left)?.ok_or_else(|| eyre!("Incomplete Gradle HTTP response"))?;
            if raw.is_empty() {
                break;
            }
            let split = raw
                .iter()
                .position(|&byte| byte == b':')
                .ok_or_else(|| eyre!("Malformed Gradle HTTP response header"))?;
            let (name, value) = (&raw[..split], raw[split + 1..].trim_ascii());
            let as_text = || std::str::from_utf8(value).map(str::to_owned);
            if name.eq_ignore_ascii_case(b"content-length") {
                ensure!(headers.length.is_none(), "Duplicate Gradle Content-Length");
                ensure!(
                    !value.is_empty() && value.iter().all(u8::is_ascii_digit),
                    "Invalid Gradle Content-Length"
                );
                headers.length = Some(std::str::from_utf8(value)?.parse()?);
            } else if name.eq_ignore_ascii_case(b"etag") {
                ensure!(headers.etag.replace(value.to_vec()).is_none(), "Duplicate Gradle ETag");
            } else if name.eq_ignore_ascii_case(b"content-range") {
                ensure!(
                    headers.range.replace(as_text()?).is_none(),
                    "Duplicate Gradle Content-Range"
                );
            } else if name.eq_ignore_ascii_case(b"content-encoding") {
                ensure!(
                    headers.encoding.replace(as_text()?).is_none(),
                    "Duplicate Gradle Content-Encoding"
                );
            } else if name.eq_ignore_ascii_case(b"location") {
                ensure!(
                    headers.location.replace(as_text()?).is_none(),
                    "Duplicate Gradle redirect location"
                );
            }
        }
        if status >= 200 {
            return Ok(Some(headers));
        }
    }
    Err(eyre!("Too many informational Gradle HTTP responses"))
}

struct Response {
    child: Child,
    pipe: BufReader<ChildStdout>,
}

impl Drop for Response {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn curl_failure(status: ExitStatus) -> Failure {
    match status.code() {
        Some(5 | 6 | 7 | 18 | 28 | 35 | 52 | 55 | 56 | 92) => {
            Failure::Retry(format!("Gradle HTTPS transport exited with {status}"))
        }
        _ => Failure::Fatal(eyre!("Gradle HTTPS transport exited with {status}")),
    }
}

fn status_ok(status: u16) -> Transfer<()> {
    match status {
        200 | 206 => Ok(()),
        408 | 429 | 500 | 502 | 503 | 504 => Err(Failure::Retry(format!("Gradle HTTP {status}"))),
        _ => Err(Failure::Fatal(eyre!("Unexpected Gradle HTTP status {status}"))),
    }
}

fn local_proxy(value: &str) -> Option<(&str, u16)> {
    let authority = value.split_once("://").map_or(value, |(_, rest)| rest);
    let (host, port) = authority.split('/').next()?.split_once(':')?;
    matches!(host, "127.0.0.1" | "localhost").then_some((host, port.parse().ok()?))
}

pub(super) struct Transport {
    bypass_proxy: bool,
}

impl Transport {
    pub(super) fn new() -> Self {
        let mut bypass_proxy = false;
        for key in PROXIES {
            let Ok(value) = std::env::var(key) else { continue };
            let Some((host, port)) = local_proxy(&value) else {
                continue;
            };
            let v4 = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
            let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, port));
            let timeout = Duration::from_secs(1);
            if TcpStream::connect_timeout(&v4, timeout).is_err()
                && (host != "localhost" || TcpStream::connect_timeout(&v6, timeout).is_err())
            {
                eprintln!("Gradle: loopback proxy {host}:{port} is offline; using direct/VIF");
                bypass_proxy = true;
                break;
            }
        }
        Self { bypass_proxy }
    }

    fn request(
        &self,
        urls: &[&str],
        requested: Option<(u64, &Representation)>,
        timeout: &str,
    ) -> Transfer<(Response, Headers)> {
        let mut url = urls[0].to_owned();
        for _ in 0..4 {
            let mut command = tool_command(Path::new("/usr/bin/curl"))?;
            if self.bypass_proxy {
                for key in PROXIES {
                    command.env_remove(key);
                }
            }
            command.args([
                "--disable",
                "--silent",
                "--include",
                "--suppress-connect-headers",
                "--proto",
                "=https",
                "--connect-timeout",
                "15",
                "--max-time",
                timeout,
                "--speed-time",
                "90",
                "--speed-limit",
                "1",
                "--header",
                "Accept-Encoding: identity",
                "--user-agent",
                "studio-gradle-nightly/1",
            ]);
            if let Some((offset, representation)) = requested {
                let mut if_range = b"If-Range: ".to_vec();
                if_range.extend_from_slice(&representation.etag);
                command
                    .args(["--header", &format!("Range: bytes={offset}-"), "--header"])
                    .arg(OsString::from_vec(if_range));
            }
            #[expect(
                clippy::disallowed_methods,
                reason = "The checked wrapper logs opaque server ETags from the If-Range argument"
            )]
            let mut child = command
                .args(["--", &url])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .wrap_err("Could not start the macOS HTTPS transport")?;
            let pipe = BufReader::new(child.stdout.take().expect("piped curl stdout"));
            let mut response = Response { child, pipe };
            let Some(headers) = headers(&mut response.pipe)? else {
                let status = response.child.wait()?;
                return Err(if status.success() {
                    Failure::Fatal(eyre!("Gradle HTTPS response was empty"))
                } else {
                    curl_failure(status)
                });
            };
            if matches!(headers.status, 301 | 302 | 303 | 307 | 308) {
                url = headers
                    .location
                    .filter(|location| urls.contains(&location.as_str()))
                    .ok_or_else(|| eyre!("Gradle redirect left the approved HTTPS URLs"))?;
                continue;
            }
            status_ok(headers.status)?;
            return Ok((response, headers));
        }
        Err(eyre!("Too many Gradle redirects").into())
    }

    pub(super) fn metadata(&self) -> Transfer<Nightly> {
        retry("Nightly metadata", || {
            let (response, headers) = self.request(&[API], None, "30")?;
            if headers.status != 200 || !identity(&headers) || headers.length.is_some_and(|n| n > MAX_METADATA) {
                return Err(eyre!("Unexpected Gradle Nightly metadata response").into());
            }
            let mut bytes = Vec::new();
            let (size, status) = body(response, MAX_METADATA, &mut bytes)?;
            if !status.success() {
                return Err(curl_failure(status));
            }
            if headers.length.is_some_and(|expected| size != expected) {
                return Err(Failure::Retry(
                    "Gradle metadata transfer ended before its declared length".into(),
                ));
            }
            serde_json::from_slice(&bytes).map_err(|_| eyre!("Invalid Gradle Nightly metadata").into())
        })
    }

    pub(super) fn download(&self, nightly: &Nightly, staging: &Path) -> Transfer<File> {
        let download = nightly
            .download_url
            .replacen("://services.gradle.org/", "://downloads.gradle.org/", 1);
        let urls = [nightly.download_url.as_str(), download.as_str()];
        let mut archive = tempfile::tempfile_in(staging)?;
        let mut partial = None;
        let mut received = 0;
        retry("distribution", || {
            let requested = partial.as_ref().map(|representation| (received, representation));
            let (response, headers) = self.request(&urls, requested, "600")?;
            let plan = body_plan(headers, requested)?;
            if plan.offset != 0 {
                println!("Gradle: resuming the same representation at byte {}", plan.offset);
            }
            let mut chunk = tempfile::tempfile_in(staging)?;
            let (size, status) = body(response, plan.length.unwrap_or(MAX_ZIP - plan.offset), &mut chunk)?;
            let incomplete = match (status.success(), plan.length) {
                (true, Some(length)) if size != length => Some(Failure::Retry(
                    "Gradle transfer ended before its declared length".into(),
                )),
                (true, _) => None,
                (false, _) => Some(curl_failure(status)),
            };
            if matches!(incomplete, Some(Failure::Fatal(_))) {
                return Err(incomplete.expect("classified fatal transfer failure"));
            }
            if plan.offset == 0 {
                archive.set_len(0)?;
            }
            archive.seek(io::SeekFrom::Start(plan.offset))?;
            chunk.rewind()?;
            io::copy(&mut chunk, &mut archive)?;
            received = plan.offset + size;
            partial = plan
                .representation
                .filter(|representation| 0 < received && received < representation.total);
            if let Some(problem) = incomplete {
                return Err(problem);
            }
            archive.rewind()?;
            let actual = sha256(archive.try_clone()?)?;
            if actual != nightly.checksum {
                return Err(eyre!("Gradle archive SHA-256 did not match the published digest").into());
            }
            archive.rewind()?;
            println!("Gradle: {received} bytes, SHA-256 verified");
            Ok(())
        })?;
        Ok(archive)
    }
}

fn retry<T>(label: &str, mut action: impl FnMut() -> Transfer<T>) -> Transfer<T> {
    for attempt in 1..=3 {
        match action() {
            Err(Failure::Retry(problem)) if attempt < 3 => {
                eprintln!("Gradle {label}: attempt {attempt}/3 failed: {problem}");
                thread::sleep(Duration::from_secs(attempt * 2));
            }
            result => return result,
        }
    }
    unreachable!("bounded retry always returns on its final attempt")
}

fn identity(headers: &Headers) -> bool {
    headers
        .encoding
        .as_ref()
        .is_none_or(|value| value.eq_ignore_ascii_case("identity"))
}

fn body(mut response: Response, max: u64, target: &mut impl io::Write) -> Transfer<(u64, ExitStatus)> {
    let size = io::copy(&mut (&mut response.pipe).take(max + 1), target)?;
    if size > max {
        return Err(eyre!("Gradle response exceeded its declared or permitted length").into());
    }
    Ok((size, response.child.wait()?))
}

#[derive(Debug)]
struct Representation {
    total: u64,
    etag: Vec<u8>,
}

struct BodyPlan {
    offset: u64,
    length: Option<u64>,
    representation: Option<Representation>,
}

fn strong_etag(bytes: &[u8]) -> bool {
    matches!(bytes, [b'"', value @ .., b'"'] if value.iter().all(|byte| matches!(byte, 0x21 | 0x23..=0x7e | 0x80..=0xff)))
}

fn body_plan(headers: Headers, requested: Option<(u64, &Representation)>) -> Result<BodyPlan> {
    ensure!(identity(&headers), "Gradle archive must use identity transfer encoding");
    if headers.status == 206 {
        let (offset, saved) = requested.ok_or_else(|| eyre!("Unexpected Gradle partial response"))?;
        ensure!(
            0 < offset
                && offset < saved.total
                && headers.etag.as_ref() == Some(&saved.etag)
                && headers.length == Some(saved.total - offset)
                && headers.range.as_deref() == Some(&format!("bytes {offset}-{}/{}", saved.total - 1, saved.total)),
            "Gradle resumed response has inconsistent range, ETag or total"
        );
        return Ok(BodyPlan {
            offset,
            length: headers.length,
            representation: headers.etag.map(|etag| Representation {
                total: saved.total,
                etag,
            }),
        });
    }
    ensure!(
        headers.status == 200 && headers.range.is_none(),
        "Unexpected full Gradle response"
    );
    ensure!(
        headers.length.is_none_or(|total| (1..=MAX_ZIP).contains(&total)),
        "Gradle archive exceeds the 1 GiB size limit or is empty"
    );
    let representation = headers
        .length
        .zip(headers.etag)
        .filter(|(_, etag)| strong_etag(etag))
        .map(|(total, etag)| Representation { total, etag });
    Ok(BodyPlan {
        offset: 0,
        length: headers.length,
        representation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_requires_exact_strong_etag_range_and_body_size() -> Result<()> {
        let full = b"HTTP/2 200\r\nContent-Length: 100\r\nETag: \"strong\"\r\n\r\nzip";
        let mut stream = full.as_slice();
        let initial = body_plan(headers(&mut stream)?.expect("full response"), None)?;
        assert_eq!(stream, b"zip");
        let saved = initial.representation.expect("strong ETag and exact total");
        let valid = b"HTTP/2 206\r\nContent-Length: 60\r\nContent-Range: bytes 40-99/100\r\nETag: \"strong\"\r\n\r\n";
        assert_eq!(
            body_plan(headers(&mut valid.as_slice())?.unwrap(), Some((40, &saved)))?.offset,
            40
        );
        let valid = std::str::from_utf8(valid)?;
        for malformed in [
            valid.replace("\"strong\"", "W/\"strong\""),
            valid.replace("40-99/100", "41-99/100"),
            valid.replace("40-99/100", "40-99/101"),
            valid.replace("Length: 60", "Length: 61"),
            valid.replace("ETag:", "Content-Encoding: gzip\r\nETag:"),
        ] {
            assert!(body_plan(headers(&mut malformed.as_bytes())?.unwrap(), Some((40, &saved))).is_err());
        }
        assert_eq!(
            body_plan(headers(&mut full.as_slice())?.unwrap(), Some((40, &saved)))?.offset,
            0
        );
        assert!(!strong_etag(b"W/\"weak\""));
        Ok(())
    }
}
