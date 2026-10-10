//! `discover` —— 列表 provider 当前提供的模型（供给每天在变，这是第一步）。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lilyco::prelude::*;
use serde_json::{json, Value};

use crate::bench;
use crate::http::Http;
use crate::keys::KeyStore;
use crate::provider;

/// 列出 provider 当前提供的模型（纯发现，不探测不测速）
#[derive(App)]
#[app(
    name = "discover",
    run = "cmd_discover",
    about = "List models a provider currently offers, before any probing or speed test. Returns { scope, providers: [{ provider, count, models }] }. Read-only."
)]
pub struct Discover {
    /// 提供商: nim / openrouter / both（缺省 both）
    provider: Option<String>,
    /// API keys 文件路径（缺省 ~/.config/lyco-llm/keys.env）
    keys: Option<PathBuf>,
    /// 模型范围: free / all（openrouter 缺省 free）
    scope: Option<String>,
    /// 请求超时秒数（缺省 15）
    #[arg(default = 15)]
    timeout: Option<u32>,
}

fn cmd_discover(app: &Discover, ctx: &Context) -> Result<Value, AppError> {
    let start = Instant::now();
    let timeout = app.timeout.unwrap_or(15).max(1) as u64;
    let http = Http::new(Duration::from_secs(timeout));
    let keystore = KeyStore::load(app.keys.as_deref())?;
    let providers = provider::parse_providers(app.provider.as_deref().unwrap_or(""))?;
    let scope = app.scope.as_deref().unwrap_or("");
    let mut per = Vec::new();
    for p in &providers {
        let auth = p.auth(&keystore)?;
        let ids = bench::discover(&http, *p, &auth, scope)?;
        ctx.log(
            LogLevel::Info,
            format!("{}: {} 个模型", p.name(), ids.len()),
        );
        per.push(json!({ "provider": p.name(), "count": ids.len(), "models": ids }));
    }
    let result = json!({ "scope": scope, "providers": per });
    ctx.done(result.clone(), start.elapsed().as_millis() as u64);
    Ok(result)
}
