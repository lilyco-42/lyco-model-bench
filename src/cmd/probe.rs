//! `probe` —— 1-token 探测 liveness：把「列表里有但一调就卡死/下线」的模型挑出来。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lilyco::prelude::*;
use serde_json::{json, Value};

use crate::bench;
use crate::http::Http;
use crate::keys::KeyStore;
use crate::provider;

/// 探测模型存活：每个模型发一次 1-token 请求，判定 live / HTTP 错误 / 超时 / 网络错误
#[derive(App)]
#[app(
    name = "probe",
    run = "cmd_probe",
    about = "Probe model liveness with a single 1-token request each. Steers around models that appear in the catalog but hang or reject. Returns { live, total, results: [{ provider, model, live, ms, reason }] }. Read-only."
)]
pub struct Probe {
    /// 提供商: nim / openrouter / both（缺省 both）
    provider: Option<String>,
    /// API keys 文件路径（缺省 ~/.config/lyco-llm/keys.env）
    keys: Option<PathBuf>,
    /// 模型范围: free / all（openrouter 缺省 free；与 --model 同用时不生效）
    scope: Option<String>,
    /// 显式探测的模型（可多次传；缺省=自动发现）
    model: Option<Vec<String>>,
    /// 单个探测超时秒数（缺省 8）
    #[arg(default = 8)]
    probe_timeout: Option<u32>,
    /// 并发数（缺省 8）
    #[arg(default = 8)]
    concurrency: Option<u32>,
}

fn cmd_probe(app: &Probe, ctx: &Context) -> Result<Value, AppError> {
    let start = Instant::now();
    let pt = app.probe_timeout.unwrap_or(8).max(1) as u64;
    let conc = app.concurrency.unwrap_or(8).max(1) as usize;
    let http = Http::new(Duration::from_secs(pt));
    let keystore = KeyStore::load(app.keys.as_deref())?;
    let providers = provider::parse_providers(app.provider.as_deref().unwrap_or(""))?;
    let scope = app.scope.as_deref().unwrap_or("");
    let explicit: Vec<String> = app.model.clone().unwrap_or_default();
    let mut probes = Vec::new();
    for p in &providers {
        let auth = p.auth(&keystore)?;
        let models: Vec<String> = if explicit.is_empty() {
            bench::discover(&http, *p, &auth, scope)?
        } else {
            explicit
                .iter()
                .filter(|m| provider::provider_for_model(m) == *p)
                .cloned()
                .collect()
        };
        ctx.log(
            LogLevel::Info,
            format!("== {}: 探测 {} 个模型", p.name(), models.len()),
        );
        probes.extend(bench::probe_many(&http, *p, &auth, &models, conc));
    }
    let live = probes.iter().filter(|r| r.live).count();
    ctx.log(
        LogLevel::Info,
        format!("live={live} dead={}", probes.len() - live),
    );
    let result = json!({ "live": live, "total": probes.len(), "results": probes });
    ctx.done(result.clone(), start.elapsed().as_millis() as u64);
    Ok(result)
}
