//! `daily` —— 每日一次：发现 → 探测 → 测速 → 落盘报告（含推荐）。

use std::path::PathBuf;
use std::time::Instant;

use lilyco::prelude::*;
use serde_json::{json, Value};

use crate::bench;
use crate::cmd::{settings, Raw};
use crate::report;

/// 每日全流程并落盘：results/{latest.json, latest.md, recommend.json}
///
/// 注意：这是本工具唯一写盘的命令（把报告写进 `--out` 目录），其余命令都是只读的。
#[derive(App)]
#[app(
    name = "daily",
    run = "cmd_daily",
    about = "Daily run: discover → probe → benchmark → write {latest.json, latest.md, recommend.json} into the results dir and return the report summary. The only command that writes files (the report dir); schedule it once a day — provider catalogs change constantly."
)]
pub struct Daily {
    /// 提供商: nim / openrouter / both（缺省 both）
    provider: Option<String>,
    /// API keys 文件路径（缺省 ~/.config/lyco-llm/keys.env）
    keys: Option<PathBuf>,
    /// 模型范围: free / all（openrouter 缺省 free）
    scope: Option<String>,
    /// 输出目录（缺省 results）
    out: Option<PathBuf>,
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

fn cmd_daily(app: &Daily, ctx: &Context) -> Result<Value, AppError> {
    let start = Instant::now();
    let s = settings(&Raw {
        provider: app.provider.clone().unwrap_or_default(),
        keys: app.keys.clone(),
        scope: app.scope.clone().unwrap_or_default(),
        models: Vec::new(),
        prompt: app.prompt.clone().unwrap_or_default(),
        max_tokens: app.max_tokens.unwrap_or(512),
        reps: app.reps.unwrap_or(3),
        timeout: app.timeout.unwrap_or(60),
        probe_timeout: app.probe_timeout.unwrap_or(8),
        concurrency: app.concurrency.unwrap_or(8),
    })?;
    let pipe = bench::pipeline(&s, ctx)?;

    let out = match app.out.as_deref() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("results"),
    };
    std::fs::create_dir_all(&out)?;
    std::fs::write(
        out.join("latest.json"),
        serde_json::to_string_pretty(&pipe)?,
    )?;
    let md = report::render_md(&pipe);
    std::fs::write(out.join("latest.md"), &md)?;
    if let Some(rec) = &pipe.recommend {
        std::fs::write(
            out.join("recommend.json"),
            serde_json::to_string_pretty(rec)?,
        )?;
    }

    let result = json!({
        "when": pipe.when.clone(),
        "out": out.to_string_lossy(),
        "rows": pipe.rows.len(),
        "dead": pipe.dead.len(),
        "recommend": pipe.recommend,
    });
    let duration = start.elapsed().as_millis() as u64;
    ctx.log(LogLevel::Info, format!("== 报告已写入 {}", out.display()));
    match &pipe.recommend {
        Some(r) => ctx.log(
            LogLevel::Info,
            format!(
                "RECOMMEND: {} {} ({:.1} tok/s)",
                r.provider,
                r.model,
                r.tokens_per_sec.unwrap_or(0.0)
            ),
        ),
        None => ctx.log(LogLevel::Info, "RECOMMEND: (none — 没有能中文回答的模型)"),
    }
    ctx.done(result.clone(), duration);
    Ok(result)
}
