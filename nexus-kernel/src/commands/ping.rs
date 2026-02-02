//! The `ping` command - send ICMP echo requests via system ping.

use super::{CommandContext, NexusCommand};
use nexus_api::{NetEventInfo, NetEventType, ShellEvent, Value};
use std::io::BufRead;
use std::process::{Command, Stdio};
use std::time::Instant;

pub struct PingCommand;

impl NexusCommand for PingCommand {
    fn name(&self) -> &'static str {
        "ping"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut count: Option<u32> = None;
        let mut host: Option<String> = None;
        let mut i = 0;

        while i < args.len() {
            match args[i].as_str() {
                "-c" | "--count" => {
                    if i + 1 < args.len() {
                        count = args[i + 1].parse().ok();
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                arg if arg.starts_with("-c") => {
                    count = arg[2..].parse().ok();
                    i += 1;
                }
                arg if !arg.starts_with('-') => {
                    host = Some(arg.to_string());
                    i += 1;
                }
                _ => i += 1,
            }
        }

        let host = host.ok_or_else(|| anyhow::anyhow!("ping: missing host operand"))?;
        let count = count.unwrap_or(4);

        let mut cmd = Command::new("ping");
        cmd.arg("-c").arg(count.to_string());
        cmd.arg(&host);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("ping: failed to capture stdout"))?;

        let reader = std::io::BufReader::new(stdout);
        let mut seq_counter: u64 = 0;
        let mut events: Vec<Value> = Vec::new();
        let mut last_emit = Instant::now();
        let mut sent: u32 = 0;
        let mut received: u32 = 0;
        let mut rtt_sum: f64 = 0.0;

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };

            if let Some(evt) = parse_ping_line(&line, &host) {
                sent += 1;
                if evt.success {
                    received += 1;
                    if let Some(rtt) = evt.rtt_ms {
                        rtt_sum += rtt;
                    }
                }

                let event_value = Value::NetEvent(Box::new(evt));
                events.push(event_value.clone());

                // Throttle: emit at most every 100ms
                if last_emit.elapsed().as_millis() >= 100 || sent == 1 || sent == count {
                    seq_counter += 1;
                    let _ = ctx.events.send(ShellEvent::StreamingUpdate {
                        block_id: ctx.block_id,
                        seq: seq_counter,
                        update: event_value,
                        coalesce: false,
                    });
                    last_emit = Instant::now();
                }
            }
        }

        let _ = child.wait();

        let loss_pct = if sent > 0 {
            ((sent - received) as f64 / sent as f64) * 100.0
        } else {
            0.0
        };
        let avg_rtt = if received > 0 { rtt_sum / received as f64 } else { 0.0 };

        Ok(Value::Record(vec![
            ("host".to_string(), Value::String(host)),
            ("sent".to_string(), Value::Int(sent as i64)),
            ("received".to_string(), Value::Int(received as i64)),
            ("loss_pct".to_string(), Value::Float(loss_pct)),
            ("avg_rtt_ms".to_string(), Value::Float(avg_rtt)),
            ("events".to_string(), Value::List(events)),
        ]))
    }
}

/// Parse a single ping output line into a NetEventInfo.
fn parse_ping_line(line: &str, host: &str) -> Option<NetEventInfo> {
    // Match lines like: "64 bytes from 8.8.8.8: icmp_seq=0 ttl=118 time=12.3 ms"
    if line.contains("bytes from") {
        let ip = line
            .split("from ")
            .nth(1)
            .and_then(|s| s.split(':').next())
            .map(|s| s.trim().to_string());

        let seq = line
            .split("icmp_seq=")
            .nth(1)
            .or_else(|| line.split("seq=").nth(1))
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<u32>().ok());

        let ttl = line
            .split("ttl=")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<u32>().ok());

        let rtt = line
            .split("time=")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<f64>().ok());

        Some(NetEventInfo {
            event_type: NetEventType::PingResponse,
            host: host.to_string(),
            ip,
            rtt_ms: rtt,
            ttl,
            seq,
            success: true,
            message: None,
        })
    } else if line.contains("Request timeout") || line.contains("timed out") {
        let seq = line
            .split("icmp_seq ")
            .nth(1)
            .or_else(|| line.split("seq ").nth(1))
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<u32>().ok());

        Some(NetEventInfo {
            event_type: NetEventType::Timeout,
            host: host.to_string(),
            ip: None,
            rtt_ms: None,
            ttl: None,
            seq,
            success: false,
            message: Some("Request timeout".to_string()),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_parse_ping_response() {
        let line = "64 bytes from 8.8.8.8: icmp_seq=0 ttl=118 time=12.3 ms";
        let evt = parse_ping_line(line, "8.8.8.8").unwrap();

        assert!(matches!(evt.event_type, NetEventType::PingResponse));
        assert!(evt.success);
        assert_eq!(evt.ip.as_deref(), Some("8.8.8.8"));
        assert_eq!(evt.seq, Some(0));
        assert_eq!(evt.ttl, Some(118));
        assert!((evt.rtt_ms.unwrap() - 12.3).abs() < 0.01);
    }

    #[test]
    fn test_parse_ping_timeout() {
        let line = "Request timeout for icmp_seq 3";
        let evt = parse_ping_line(line, "example.com").unwrap();

        assert!(matches!(evt.event_type, NetEventType::Timeout));
        assert!(!evt.success);
        assert_eq!(evt.seq, Some(3));
        assert!(evt.rtt_ms.is_none());
    }

    #[test]
    fn test_parse_ping_irrelevant_line() {
        let line = "PING 8.8.8.8 (8.8.8.8): 56 data bytes";
        assert!(parse_ping_line(line, "8.8.8.8").is_none());
    }

    #[test]
    fn test_parse_ping_linux_format() {
        let line = "64 bytes from 8.8.8.8: icmp_seq=1 ttl=64 time=0.042 ms";
        let evt = parse_ping_line(line, "8.8.8.8").unwrap();

        assert!(evt.success);
        assert_eq!(evt.seq, Some(1));
        assert!((evt.rtt_ms.unwrap() - 0.042).abs() < 0.001);
    }

    #[test]
    fn test_ping_missing_host() {
        let mut test_ctx = TestContext::new_default();
        let cmd = PingCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx());
        assert!(result.is_err());
    }
}
