//! The `dig` command - DNS lookup with structured output.
//!
//! Shells out to the system `dig` command and parses the output into
//! structured `DnsAnswerInfo` values.

use super::{CommandContext, NexusCommand};
use nexus_api::{DnsAnswerInfo, DnsRecord, Value};
use std::process::Command;
use std::time::Instant;

pub struct DigCommand;

impl NexusCommand for DigCommand {
    fn name(&self) -> &'static str {
        "dig"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut query: Option<String> = None;
        let mut record_type = "A".to_string();
        let mut server: Option<String> = None;
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];
            match arg.as_str() {
                "A" | "AAAA" | "MX" | "TXT" | "CNAME" | "NS" | "SOA" | "SRV" | "PTR" => {
                    record_type = arg.to_string();
                    i += 1;
                }
                "-t" | "--type" => {
                    if i + 1 < args.len() {
                        record_type = args[i + 1].to_uppercase();
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                arg if arg.starts_with('@') => {
                    server = Some(arg[1..].to_string());
                    i += 1;
                }
                arg if !arg.starts_with('-') => {
                    query = Some(arg.to_string());
                    i += 1;
                }
                _ => i += 1,
            }
        }

        let query = query.ok_or_else(|| anyhow::anyhow!("dig: missing domain name"))?;

        let start = Instant::now();

        let mut cmd = Command::new("dig");
        cmd.arg(&query).arg(&record_type).arg("+noall").arg("+answer").arg("+stats");
        if let Some(ref srv) = server {
            cmd.arg(format!("@{}", srv));
        }

        let output = cmd.output()?;
        let query_time_ms = start.elapsed().as_secs_f64() * 1000.0;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("dig: {}", stderr.trim()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut answers: Vec<DnsRecord> = Vec::new();
        let mut dig_server = server.unwrap_or_else(|| "system default".to_string());
        let mut dig_query_time: Option<f64> = None;

        for line in stdout.lines() {
            let line = line.trim();

            // Parse answer lines: name TTL IN TYPE DATA
            if !line.starts_with(';') && !line.is_empty() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 5 && parts[2] == "IN" {
                    answers.push(DnsRecord {
                        name: parts[0].to_string(),
                        record_type: parts[3].to_string(),
                        ttl: parts[1].parse().unwrap_or(0),
                        data: parts[4..].join(" "),
                    });
                }
            }

            // Parse stats
            if line.starts_with(";; Query time:") {
                dig_query_time = line
                    .split("Query time:")
                    .nth(1)
                    .and_then(|s| s.trim().split_whitespace().next())
                    .and_then(|s| s.parse().ok());
            }
            if line.starts_with(";; SERVER:") {
                if let Some(s) = line.split("SERVER:").nth(1) {
                    dig_server = s.trim().to_string();
                }
            }
        }

        Ok(Value::dns_answer(DnsAnswerInfo {
            query,
            record_type,
            answers,
            query_time_ms: dig_query_time.unwrap_or(query_time_ms),
            server: dig_server,
            from_cache: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_dig_missing_domain() {
        let mut test_ctx = TestContext::new_default();
        let cmd = DigCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx());
        assert!(result.is_err());
    }

    #[test]
    fn test_dig_localhost() {
        let mut test_ctx = TestContext::new_default();
        let cmd = DigCommand;
        let result = cmd
            .execute(&["localhost".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result.as_domain() {
            Some(nexus_api::DomainValue::DnsAnswer(info)) => {
                assert_eq!(info.query, "localhost");
                assert_eq!(info.record_type, "A");
            }
            _ => panic!("Expected DnsAnswer"),
        }
    }

    #[test]
    fn test_dig_record_type() {
        let mut test_ctx = TestContext::new_default();
        let cmd = DigCommand;
        let result = cmd
            .execute(
                &["localhost".to_string(), "AAAA".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result.as_domain() {
            Some(nexus_api::DomainValue::DnsAnswer(info)) => {
                assert_eq!(info.record_type, "AAAA");
            }
            _ => panic!("Expected DnsAnswer"),
        }
    }
}
