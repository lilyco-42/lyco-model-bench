//! `report` —— 把 `daily` 落盘的 latest.json 重新渲染成 Markdown（只读）。

use std::path::PathBuf;

use lilyco::prelude::*;
use serde_json::{json, Value};

use crate::bench::Pipeline;
use crate::report;

/// 把 daily 落盘的 latest.json 渲染成 Markdown（stdout + 返回）
#[derive(App)]
#[app(
    name = "report",
    run = "cmd_report",
    about = "Render the saved Pipeline JSON (default results/latest.json) as Markdown, without touching the network. Read-only."
)]
pub struct Report {
    /// 输入 JSON 路径（缺省 results/latest.json）
    json: Option<PathBuf>,
}

fn cmd_report(app: &Report, ctx: &Context) -> Result<Value, AppError> {
    let path = match app.json.as_deref() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("results").join("latest.json"),
    };
    let text = std::fs::read_to_string(&path)?;
    let pipe: Pipeline = serde_json::from_str(&text)?;
    let md = report::render_md(&pipe);
    // 人类看 stdout 的 Markdown；机器要的放返回值里
    ctx.log(LogLevel::Info, md.clone());
    let result = json!({ "source": path.to_string_lossy(), "markdown": md });
    ctx.done(result.clone(), 0);
    Ok(result)
}
