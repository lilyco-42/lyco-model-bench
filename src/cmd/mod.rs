//! 命令：一个 struct = 一条命令，四端（CLI/TUI/Web/MCP）共用同一 schema 与 handler。

mod bench;
mod daily;
mod discover;
mod probe;
mod report;

pub use bench::Bench;
pub use daily::Daily;
pub use discover::Discover;
pub use probe::Probe;
pub use report::Report;

use std::path::PathBuf;
use std::time::Duration;

use lilyco::prelude::AppError;

use crate::bench::{Settings, DEFAULT_PROMPT};
use crate::keys::KeyStore;
use crate::provider;

/// bench / daily 共享的参数快照（各命令把 Option 字段解包后填这里）。
pub(crate) struct Raw {
    pub provider: String,
    pub keys: Option<PathBuf>,
    pub scope: String,
    pub models: Vec<String>,
    pub prompt: String,
    pub max_tokens: u32,
    pub reps: u32,
    pub timeout: u32,
    pub probe_timeout: u32,
    pub concurrency: u32,
}

/// 把公共参数解析成 [`Settings`]：解析 provider、装载密钥、换算超时。
pub(crate) fn settings(r: &Raw) -> Result<Settings, AppError> {
    let providers = provider::parse_providers(&r.provider)?;
    let keystore = KeyStore::load(r.keys.as_deref())?;
    let mut auths = Vec::new();
    for p in &providers {
        auths.push((*p, p.auth(&keystore)?));
    }
    Ok(Settings {
        providers,
        auths,
        scope: r.scope.clone(),
        models: r.models.clone(),
        max_tokens: r.max_tokens,
        reps: r.reps,
        bench_timeout: Duration::from_secs(u64::from(r.timeout.max(1))),
        probe_timeout: Duration::from_secs(u64::from(r.probe_timeout.max(1))),
        concurrency: r.concurrency.max(1) as usize,
        prompt: if r.prompt.is_empty() {
            DEFAULT_PROMPT.to_string()
        } else {
            r.prompt.clone()
        },
    })
}
