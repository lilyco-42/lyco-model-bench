//! `bench` —— 全流程测速：发现 → 探测 → 用固定中文提示实测 token 速度。

use std::path::PathBuf;
use std::time::Instant;

use lilyco::prelude::*;
use serde_json::Value;

use crate::bench;
use crate::cmd::{settings, Raw};

/// 发现 → 探测 → 测速：用固定中文提示对存活的模型重复测速，返回整份报告（不落盘）
#[derive(App)]
#[app(
    name = "bench",
    run = "cmd_bench",
    about = "Full speed pipeline: discover → probe → benchmark live models with a fixed Chinese prompt (reps runs, median). Returns the Pipeline JSON: rows with tokens/s, TTFT, answered/chinese flags, plus slow/dead list. Read-only."
)]
pub struct Bench {
    /// 提供商: nim / openrouter / both（缺省 both）
    provider: Option<String>,
    /// API keys 文件路径（缺省 ~/.config/lyco-llm/keys.env）
    keys: Option<PathBuf>,
    /// 模型范围: free / all（openrouter 缺省 free；与 --model 同用时不生效）
    scope: Option<String>,
    /// 显式测速的模型（可多次传；缺省=自动发现+探测）
    model: Option<Vec<String>>,
    /// 单次生成长度上限（缺省 512；0=用缺省）
    #[arg(default = 512)]
    max_tokens: Option<u32>,
    /// 重复测速取中位数（缺省 3）
    #[arg(default = 3)]
    reps: Option<u32>,
    /// 测速超时秒数（缺省 60）
    #[arg(default = 60)]
    timeout: Option<u32>,
    /// 探测超时秒数（缺省 8）
    #[arg(default = 8)]
    probe_timeout: Option<u32>,
    /// 并发数（缺省 8）
    #[arg(default = 8)]
    concurrency: Option<u32>,
    /// 提示词（缺省内置中文提示）
    prompt: Option<String>,
}

fn cmd_bench(app: &Bench, ctx: &Context) -> Result<Value, AppError> {
    let start = Instant::now();
    let s = settings(&Raw {
        provider: app.provider.clone().unwrap_or_default(),
        keys: app.keys.clone(),
        scope: app.scope.clone().unwrap_or_default(),
        models: app.model.clone().unwrap_or_default(),
        prompt: app.prompt.clone().unwrap_or_default(),
        max_tokens: app.max_tokens.unwrap_or(512),
        reps: app.reps.unwrap_or(3),
        timeout: app.timeout.unwrap_or(60),
        probe_timeout: app.probe_timeout.unwrap_or(8),
        concurrency: app.concurrency.unwrap_or(8),
    })?;
    let pipe = bench::pipeline(&s, ctx)?;
    let result = serde_json::to_value(&pipe)?;
    ctx.done(result.clone(), start.elapsed().as_millis() as u64);
    Ok(result)
}
