//! The `curl` command - HTTP client with structured output.
//!
//! Shells out to system `curl` to avoid tokio runtime nesting issues
//! (reqwest::blocking creates its own internal runtime).

use super::{CommandContext, NexusCommand};
use nexus_api::{HttpResponseInfo, HttpTiming, Value};
use std::process::{Command, Stdio};

pub struct CurlCommand;

impl NexusCommand for CurlCommand {
    fn name(&self) -> &'static str {
        "curl"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut url: Option<String> = None;
        let mut method = "GET".to_string();
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut follow_redirects = false;
        let mut i = 0;

        while i < args.len() {
            match args[i].as_str() {
                "-X" | "--request" => {
                    if i + 1 < args.len() {
                        method = args[i + 1].to_uppercase();
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "-H" | "--header" => {
                    if i + 1 < args.len() {
                        if let Some((name, value)) = args[i + 1].split_once(':') {
                            headers.push((name.trim().to_string(), value.trim().to_string()));
                        }
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "-I" | "--head" => {
                    method = "HEAD".to_string();
                    i += 1;
                }
                "-L" | "--location" => {
                    follow_redirects = true;
                    i += 1;
                }
                arg if !arg.starts_with('-') => {
                    url = Some(arg.to_string());
                    i += 1;
                }
                _ => i += 1,
            }
        }

        let url = url.ok_or_else(|| anyhow::anyhow!("curl: missing URL"))?;

        let start = std::time::Instant::now();

        // Build system curl command with -i (include headers) and -s (silent)
        let mut cmd = Command::new("curl");
        cmd.arg("-s") // silent (no progress meter)
            .arg("-i") // include response headers in output
            .arg("-X")
            .arg(&method);

        if follow_redirects {
            cmd.arg("-L");
        }

        for (name, value) in &headers {
            cmd.arg("-H").arg(format!("{}: {}", name, value));
        }

        cmd.arg(&url);

        // Write timing data to stderr via %{stderr} to avoid body collision
        cmd.arg("-w")
            .arg("%{stderr}__NEXUS_TIMING__%{time_namelookup}|%{time_connect}|%{time_appconnect}|%{time_starttransfer}|%{time_total}");

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd.spawn()?.wait_with_output()?;
        let total_ms = start.elapsed().as_secs_f64() * 1000.0;

        // Extract timing from stderr (appended by -w via %{stderr})
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        let curl_timing = extract_timing_from_stderr(&stderr_str);

        // Check for curl errors (timing marker removed from stderr for error check)
        let stderr_clean = stderr_str
            .find("__NEXUS_TIMING__")
            .map(|pos| &stderr_str[..pos])
            .unwrap_or(&stderr_str);
        if !output.status.success() && output.stdout.is_empty() {
            let err = stderr_clean.trim();
            if !err.is_empty() {
                anyhow::bail!("curl: {}", err);
            }
            anyhow::bail!("curl: request failed");
        }

        let raw = output.stdout;
        let (header_bytes, body_bytes) = split_headers_body(&raw);

        // Parse status line and headers
        let header_text = String::from_utf8_lossy(header_bytes);
        let mut lines = header_text.lines();

        let (status_code, status_text) = if let Some(status_line) = lines.next() {
            parse_status_line(status_line)
        } else {
            (0, String::new())
        };

        let mut resp_headers: Vec<(String, String)> = Vec::new();
        let mut content_type: Option<String> = None;
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((name, value)) = line.split_once(':') {
                let name = name.trim().to_string();
                let value = value.trim().to_string();
                if name.eq_ignore_ascii_case("content-type") {
                    content_type = Some(value.clone());
                }
                resp_headers.push((name, value));
            }
        }

        let body_len = body_bytes.len() as u64;

        // Preview: first 4KB of text bodies
        let is_text = content_type
            .as_ref()
            .map(|ct| {
                ct.starts_with("text/")
                    || ct.contains("json")
                    || ct.contains("xml")
                    || ct.contains("javascript")
            })
            .unwrap_or(false);

        let (body_preview, body_truncated) = if is_text {
            let preview_bytes = &body_bytes[..body_bytes.len().min(4096)];
            let preview = String::from_utf8_lossy(preview_bytes).to_string();
            let truncated = body_bytes.len() > 4096;
            (Some(preview), truncated)
        } else {
            (None, false)
        };

        Ok(Value::http_response(HttpResponseInfo {
            url,
            method,
            status_code,
            status_text,
            headers: resp_headers,
            body_preview,
            body_len,
            body_truncated,
            content_type,
            timing: curl_timing.unwrap_or(HttpTiming {
                total_ms,
                dns_ms: None,
                connect_ms: None,
                tls_ms: None,
                ttfb_ms: None,
                transfer_ms: None,
            }),
        }))
    }
}

const TIMING_MARKER: &str = "__NEXUS_TIMING__";

/// Extract timing data from curl's stderr output (written via `%{stderr}` in `-w`).
fn extract_timing_from_stderr(stderr: &str) -> Option<HttpTiming> {
    let timing_data = stderr.find(TIMING_MARKER)?;
    let after_marker = &stderr[timing_data + TIMING_MARKER.len()..];
    let parts: Vec<&str> = after_marker.trim().split('|').collect();

    if parts.len() != 5 {
        return None;
    }

    let t_dns: f64 = parts[0].parse().unwrap_or(0.0);
    let t_conn: f64 = parts[1].parse().unwrap_or(0.0);
    let t_tls: f64 = parts[2].parse().unwrap_or(0.0);
    let t_ttfb: f64 = parts[3].parse().unwrap_or(0.0);
    let t_total: f64 = parts[4].parse().unwrap_or(0.0);

    let dns_ms = t_dns * 1000.0;
    let connect_ms = (t_conn - t_dns).max(0.0) * 1000.0;
    let tls_ms = if t_tls > t_conn {
        Some((t_tls - t_conn) * 1000.0)
    } else {
        None
    };
    let tls_end = if t_tls > t_conn { t_tls } else { t_conn };
    let ttfb_ms = (t_ttfb - tls_end).max(0.0) * 1000.0;
    let transfer_ms = (t_total - t_ttfb).max(0.0) * 1000.0;

    Some(HttpTiming {
        total_ms: t_total * 1000.0,
        dns_ms: Some(dns_ms),
        connect_ms: Some(connect_ms),
        tls_ms,
        ttfb_ms: Some(ttfb_ms),
        transfer_ms: Some(transfer_ms),
    })
}

/// Split raw curl -i output into (headers, body) at the first \r\n\r\n boundary.
fn split_headers_body(raw: &[u8]) -> (&[u8], &[u8]) {
    // Look for \r\n\r\n
    if let Some(pos) = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
    {
        return (&raw[..pos], &raw[pos + 4..]);
    }
    // Fallback: \n\n
    if let Some(pos) = raw
        .windows(2)
        .position(|w| w == b"\n\n")
    {
        return (&raw[..pos], &raw[pos + 2..]);
    }
    // No separator found — treat everything as headers
    (raw, &[])
}

/// Parse "HTTP/1.1 200 OK" -> (200, "OK")
fn parse_status_line(line: &str) -> (u16, String) {
    let parts: Vec<&str> = line.splitn(3, ' ').collect();
    if parts.len() >= 2 {
        let code = parts[1].parse::<u16>().unwrap_or(0);
        let text = if parts.len() >= 3 {
            parts[2].to_string()
        } else {
            String::new()
        };
        (code, text)
    } else {
        (0, String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_parse_status_line_200() {
        let (code, text) = parse_status_line("HTTP/1.1 200 OK");
        assert_eq!(code, 200);
        assert_eq!(text, "OK");
    }

    #[test]
    fn test_parse_status_line_404() {
        let (code, text) = parse_status_line("HTTP/1.1 404 Not Found");
        assert_eq!(code, 404);
        assert_eq!(text, "Not Found");
    }

    #[test]
    fn test_parse_status_line_no_reason() {
        let (code, text) = parse_status_line("HTTP/2 200");
        assert_eq!(code, 200);
        assert_eq!(text, "");
    }

    #[test]
    fn test_split_headers_body_crlf() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html>body</html>";
        let (headers, body) = split_headers_body(raw);
        assert_eq!(headers, b"HTTP/1.1 200 OK\r\nContent-Type: text/html");
        assert_eq!(body, b"<html>body</html>");
    }

    #[test]
    fn test_split_headers_body_lf() {
        let raw = b"HTTP/1.1 200 OK\nContent-Type: text/html\n\nbody";
        let (headers, body) = split_headers_body(raw);
        assert_eq!(headers, b"HTTP/1.1 200 OK\nContent-Type: text/html");
        assert_eq!(body, b"body");
    }

    #[test]
    fn test_split_headers_body_no_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 0";
        let (headers, body) = split_headers_body(raw);
        assert_eq!(headers, raw.as_slice());
        assert!(body.is_empty());
    }

    #[test]
    fn test_curl_missing_url() {
        let mut test_ctx = TestContext::new_default();
        let cmd = CurlCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx());
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_timing_from_stderr_tls() {
        let stderr = "__NEXUS_TIMING__0.012|0.045|0.120|0.200|0.350";
        let timing = extract_timing_from_stderr(stderr).unwrap();
        assert!((timing.total_ms - 350.0).abs() < 0.1);
        assert!((timing.dns_ms.unwrap() - 12.0).abs() < 0.1);
        assert!((timing.connect_ms.unwrap() - 33.0).abs() < 0.1);
        assert!((timing.tls_ms.unwrap() - 75.0).abs() < 0.1);
        assert!((timing.ttfb_ms.unwrap() - 80.0).abs() < 0.1);
        assert!((timing.transfer_ms.unwrap() - 150.0).abs() < 0.1);
    }

    #[test]
    fn test_extract_timing_from_stderr_no_tls() {
        let stderr = "__NEXUS_TIMING__0.005|0.020|0.020|0.100|0.250";
        let timing = extract_timing_from_stderr(stderr).unwrap();
        assert!(timing.tls_ms.is_none());
        assert!((timing.ttfb_ms.unwrap() - 80.0).abs() < 0.1);
    }

    #[test]
    fn test_extract_timing_from_stderr_with_errors() {
        let stderr = "curl: (6) Could not resolve host\n__NEXUS_TIMING__0.010|0.000|0.000|0.000|0.010";
        let timing = extract_timing_from_stderr(stderr).unwrap();
        assert!((timing.total_ms - 10.0).abs() < 0.1);
    }

    #[test]
    fn test_extract_timing_from_stderr_missing() {
        let stderr = "curl: (6) Could not resolve host";
        assert!(extract_timing_from_stderr(stderr).is_none());
    }
}
