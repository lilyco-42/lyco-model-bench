//! HTTP 客户端：同步、纯 Rust（ureq + rustls），只覆盖本工具需要的两种调用
//! —— GET JSON（/models）与 POST 流式 chat（/chat/completions）。

use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant};

use lilyco::prelude::AppError;

use crate::bench::{ingest, StreamAcc};

pub struct Http {
    agent: ureq::Agent,
}

impl Http {
    pub fn new(timeout: Duration) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(timeout)
            .user_agent(concat!("lyco-model-bench/", env!("CARGO_PKG_VERSION")))
            .build();
        Self { agent }
    }

    /// GET 一个 JSON 端点（目前用于各 provider 的 `/models`）。
    pub fn get_json(
        &self,
        url: &str,
        auth: &(String, String),
    ) -> Result<serde_json::Value, AppError> {
        let resp = self
            .agent
            .get(url)
            .set(&auth.0, &auth.1)
            .call()
            .map_err(http_err)?;
        let text = resp
            .into_string()
            .map_err(|e| AppError::Runtime(format!("读响应失败: {e}")))?;
        serde_json::from_str(&text)
            .map_err(|e| AppError::Runtime(format!("响应不是合法 JSON: {e}")))
    }

    /// 1-token 非流式探测（liveness）：成功、HTTP 错误、超时、网络错误四类。
    pub fn probe_model(&self, base: &str, auth: &(String, String), model: &str) -> ProbeOutcome {
        let body = serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 1,
            "temperature": 0.0,
        });
        let url = format!("{base}/chat/completions");
        let start = Instant::now();
        match self
            .agent
            .post(&url)
            .set(&auth.0, &auth.1)
            .send_string(&body.to_string())
        {
            Ok(resp) if resp.status() == 200 => ProbeOutcome::Ok {
                ms: start.elapsed().as_millis() as u64,
            },
            Ok(resp) => ProbeOutcome::Http {
                code: resp.status(),
                ms: start.elapsed().as_millis() as u64,
            },
            Err(ureq::Error::Status(code, _)) => ProbeOutcome::Http {
                code,
                ms: start.elapsed().as_millis() as u64,
            },
            Err(ureq::Error::Transport(t)) => {
                let msg = t.to_string().to_ascii_lowercase();
                if msg.contains("timeout") || msg.contains("timed out") {
                    ProbeOutcome::Timeout
                } else {
                    ProbeOutcome::NetErr(t.to_string())
                }
            }
        }
    }

    /// 流式 chat 一次完整往返：逐行消费 SSE，返回累积的流与总耗时。
    pub fn chat_stream(
        &self,
        base: &str,
        auth: &(String, String),
        model: &str,
        prompt: &str,
        max_tokens: u32,
    ) -> Result<StreamAcc, AppError> {
        let body = serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": max_tokens,
            "stream": true,
            "temperature": 0.0,
        });
        let url = format!("{base}/chat/completions");
        let start = Instant::now();
        let resp = self
            .agent
            .post(&url)
            .set(&auth.0, &auth.1)
            .send_string(&body.to_string())
            .map_err(http_err)?;
        let mut buf = BufReader::new(resp.into_reader());
        let mut line = String::new();
        let mut acc = StreamAcc::default();
        loop {
            line.clear();
            let n = buf
                .read_line(&mut line)
                .map_err(|e| AppError::Runtime(format!("读流失败: {e}")))?;
            if n == 0 {
                break;
            }
            let elapsed = start.elapsed().as_millis() as u64;
            if ingest(&line, elapsed, &mut acc) {
                break;
            }
        }
        acc.total_ms = start.elapsed().as_millis() as u64;
        Ok(acc)
    }
}

/// 探测结果分类。
pub enum ProbeOutcome {
    Ok { ms: u64 },
    Http { code: u16, ms: u64 },
    Timeout,
    NetErr(String),
}

fn http_err(e: ureq::Error) -> AppError {
    match e {
        ureq::Error::Status(code, resp) => {
            let msg = resp.into_string().unwrap_or_default();
            AppError::Runtime(format!("HTTP {code}: {}", squish(&msg, 160)))
        }
        ureq::Error::Transport(t) => AppError::Runtime(format!("网络错误: {t}")),
    }
}

fn squish(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\n' | '\r'))
        .take(max)
        .collect()
}
