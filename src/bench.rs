//! 核心流程：发现（discover）→ 探测（probe）→ 测速（bench）→ 推荐（recommend）。
//!
//! 命令只做参数整理与输出，网络与统计全部在这里；纯函数（SSE 行解析、中文判定、
//! token 估算、中位数、推荐、UTC 时间）全部带离线单测，CI 无需真实网络。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use lilyco::prelude::{AppError, Context, LogLevel};
use serde::{Deserialize, Serialize};

use crate::http::{Http, ProbeOutcome};
use crate::provider::{filter_models, provider_for_model, Provider};

/// 默认测速提示词：固定中文提示，「能不能中文回答」本身就是质量维度。
pub const DEFAULT_PROMPT: &str = "用简体中文写一段描写春天西湖的短文，大约120字。";

// ── 流累积 / SSE 行解析 ──────────────────────────────────

/// 一条流的累积状态，SSE 行逐条喂给 [`ingest`]。
#[derive(Debug, Default, Clone)]
pub struct StreamAcc {
    /// 可见正文（`delta.content` 增量拼接）
    pub content: String,
    /// 隐藏思考（`delta.reasoning_content` 增量）——NIM/OpenRouter 的部分推理模型
    /// 会整段占用预算只吐思考、不给正文；单独统计以暴露「钱花在思考上」的真因。
    pub reasoning: String,
    /// 第一个可见 token 到达的时刻（距请求开始）
    pub ttft_ms: Option<u64>,
    /// 整段请求总耗时（由调用方回填）
    pub total_ms: u64,
    pub finish: Option<String>,
    pub completion_tokens: Option<u64>,
    pub prompt_tokens: Option<u64>,
}

/// 消费一行 SSE 文本。返回 `true` 表示收到 `data: [DONE]`（流已结束）。
/// 对 `event:` / 空行 / 不可解析的行一律静默跳过（部分 provider 会发 keep-alive 与空 buffer）。
pub fn ingest(line: &str, elapsed_ms: u64, acc: &mut StreamAcc) -> bool {
    let data = match line.trim().strip_prefix("data:") {
        Some(rest) => rest.trim(),
        None => return false,
    };
    if data == "[DONE]" {
        return true;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
        return false;
    };
    if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
        if let Some(ct) = usage.get("completion_tokens").and_then(|x| x.as_u64()) {
            acc.completion_tokens = Some(ct);
        }
        if let Some(pt) = usage.get("prompt_tokens").and_then(|x| x.as_u64()) {
            acc.prompt_tokens = Some(pt);
        }
    }
    let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else {
        return false;
    };
    if let Some(fr) = choice.get("finish_reason").and_then(|x| x.as_str()) {
        acc.finish = Some(fr.to_string());
    }
    if let Some(delta) = choice.get("delta") {
        if let Some(r) = delta.get("reasoning_content").and_then(|x| x.as_str()) {
            acc.reasoning.push_str(r);
        }
        if let Some(c) = delta.get("content").and_then(|x| x.as_str()) {
            if !c.is_empty() {
                if acc.ttft_ms.is_none() {
                    acc.ttft_ms = Some(elapsed_ms);
                }
                acc.content.push_str(c);
            }
        }
    }
    // 个别网关无视 stream=true、整段给 `message.content`：仅在还没累积到内容时兜底，
    // 避免与 delta 重复计数。
    if acc.content.is_empty() {
        if let Some(c) = choice
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|x| x.as_str())
        {
            if !c.is_empty() {
                if acc.ttft_ms.is_none() {
                    acc.ttft_ms = Some(elapsed_ms);
                }
                acc.content.push_str(c);
            }
        }
    }
    false
}

// ── 中文字符与 token 估算 ────────────────────────────────

pub fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}')
}

/// 正文里是否出现中文（回答质量的粗筛）。
pub fn has_chinese(s: &str) -> bool {
    s.chars().any(is_cjk)
}

/// 无 usage 时的 token 估算：CJK 字 ≈ 1 token，其余按 4 字符 ≈ 1 token。
pub fn estimate_tokens(s: &str) -> u64 {
    let mut cjk = 0u64;
    let mut other = 0u64;
    for c in s.chars() {
        if is_cjk(c) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    cjk + other.div_ceil(4)
}

/// 结案 token 数：优先 usage，缺失时用估算。
fn resolved_tokens(acc: &StreamAcc) -> Option<u64> {
    acc.completion_tokens.or_else(|| {
        let c = acc.content.chars().count();
        (c > 0).then(|| estimate_tokens(&acc.content))
    })
}

/// tokens/s：结案 token 数 ÷ 总耗时。
pub fn tokens_per_sec(acc: &StreamAcc) -> Option<f64> {
    let tokens = resolved_tokens(acc)?;
    if acc.total_ms == 0 {
        return None;
    }
    Some(tokens as f64 * 1000.0 / acc.total_ms as f64)
}

fn median(v: &mut [u64]) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2
    })
}

// ── 行数据模型 ────────────────────────────────────────────

/// 一条测速结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    pub provider: String,
    pub model: String,
    pub ok: bool,
    pub live: bool,
    pub err: Option<String>,
    pub answered: bool,
    pub chinese: bool,
    pub tokens: Option<u64>,
    pub tokens_estimated: bool,
    pub ttft_ms: Option<u64>,
    pub total_ms: Option<u64>,
    pub tokens_per_sec: Option<f64>,
    pub content_chars: usize,
    pub reasoning_chars: usize,
    pub answer_preview: String,
    pub finish: Option<String>,
}

impl Row {
    pub fn from_acc(provider: Provider, model: &str, acc: &StreamAcc) -> Self {
        let answered = !acc.content.trim().is_empty();
        let flat: String = acc
            .content
            .chars()
            .map(|c| if c == '\n' { ' ' } else { c })
            .collect();
        let preview: String = flat.trim().chars().take(60).collect();
        Row {
            provider: provider.name().into(),
            model: model.into(),
            ok: true,
            live: true,
            err: None,
            answered,
            chinese: has_chinese(&acc.content),
            tokens: resolved_tokens(acc),
            tokens_estimated: acc.completion_tokens.is_none() && !acc.content.is_empty(),
            ttft_ms: acc.ttft_ms,
            total_ms: Some(acc.total_ms),
            tokens_per_sec: tokens_per_sec(acc),
            content_chars: acc.content.chars().count(),
            reasoning_chars: acc.reasoning.chars().count(),
            answer_preview: preview,
            finish: acc.finish.clone(),
        }
    }

    pub fn failed(provider: Provider, model: &str, err: String) -> Self {
        Row {
            provider: provider.name().into(),
            model: model.into(),
            ok: false,
            live: false,
            err: Some(err),
            answered: false,
            chinese: false,
            tokens: None,
            tokens_estimated: false,
            ttft_ms: None,
            total_ms: None,
            tokens_per_sec: None,
            content_chars: 0,
            reasoning_chars: 0,
            answer_preview: String::new(),
            finish: None,
        }
    }
}

/// 一次探测的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeRow {
    pub provider: String,
    pub model: String,
    pub live: bool,
    pub ms: Option<u64>,
    pub reason: Option<String>,
}

// ── 整份报告 ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipeline {
    pub when: String,
    pub scope: String,
    /// (provider, 候选模型数)
    pub discovered: Vec<(String, usize)>,
    pub rows: Vec<Row>,
    /// 探测未通过（卡死 / 下线 / 无权限）的模型
    pub dead: Vec<ProbeRow>,
    /// 推荐（最快的「能中文回答」模型）
    pub recommend: Option<Rec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rec {
    pub provider: String,
    pub model: String,
    pub tokens_per_sec: Option<f64>,
    pub total_ms: Option<u64>,
}

/// 推荐口径：能答、中文、测出速度里最快的。
pub fn recommend(rows: &[Row]) -> Option<Rec> {
    rows.iter()
        .filter(|r| r.ok && r.answered && r.chinese && r.tokens_per_sec.is_some())
        .max_by(|a, b| {
            a.tokens_per_sec
                .partial_cmp(&b.tokens_per_sec)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|r| Rec {
            provider: r.provider.clone(),
            model: r.model.clone(),
            tokens_per_sec: r.tokens_per_sec,
            total_ms: r.total_ms,
        })
}

// ── 流程编排 ──────────────────────────────────────────────

pub struct Settings {
    pub providers: Vec<Provider>,
    pub auths: Vec<(Provider, (String, String))>,
    pub scope: String,
    /// 显式模型列表；空 = 自动发现 + 探测
    pub models: Vec<String>,
    pub max_tokens: u32,
    pub reps: u32,
    pub bench_timeout: std::time::Duration,
    pub probe_timeout: std::time::Duration,
    pub concurrency: usize,
    pub prompt: String,
}

/// 跑完整管线：对每个 provider 发现 → 探测 → 测速，产出 [`Pipeline`]。
pub fn pipeline(s: &Settings, ctx: &Context) -> Result<Pipeline, AppError> {
    let started = Instant::now();
    let bench_http = Http::new(s.bench_timeout);
    let probe_http = Http::new(s.probe_timeout);
    let mut jobs: Vec<(Provider, String)> = Vec::new();
    let mut dead: Vec<ProbeRow> = Vec::new();
    let mut discovered: Vec<(String, usize)> = Vec::new();

    for p in &s.providers {
        let auth = auth_for(&s.auths, *p)?.clone();
        if s.models.is_empty() {
            let all = discover(&bench_http, *p, &auth, &s.scope)?;
            discovered.push((p.name().to_string(), all.len()));
            ctx.log(
                LogLevel::Info,
                format!("== {}: {} 个候选", p.name(), all.len()),
            );
            if all.is_empty() {
                continue;
            }
            ctx.log(
                LogLevel::Info,
                format!(
                    "-- probing {} 个模型 (timeout {}s)...",
                    all.len(),
                    s.probe_timeout.as_secs()
                ),
            );
            let probes = probe_many(&probe_http, *p, &auth, &all, s.concurrency);
            let live: Vec<ProbeRow> = probes
                .iter()
                .filter(|r| r.live)
                .cloned()
                .collect::<Vec<_>>();
            for r in probes.iter().filter(|r| !r.live) {
                dead.push(r.clone());
            }
            ctx.log(
                LogLevel::Info,
                format!("   live={} dead={}", live.len(), dead.len()),
            );
            for r in live {
                jobs.push((*p, r.model));
            }
        } else {
            for m in s.models.iter().filter(|m| provider_for_model(m) == *p) {
                jobs.push((*p, m.clone()));
            }
        }
    }

    ctx.log(
        LogLevel::Info,
        format!(
            "-- benchmarking {} 个模型 (reps={}, max_tokens={}, concurrency={})...",
            jobs.len(),
            s.reps,
            s.max_tokens,
            s.concurrency
        ),
    );
    let rows = bench_many(&bench_http, &s.auths, &jobs, s, ctx);
    let rec = recommend(&rows);
    let when = now_utc();
    ctx.log(
        LogLevel::Info,
        format!(
            "== 完成 {} 条，共 {}ms",
            rows.len(),
            started.elapsed().as_millis()
        ),
    );
    Ok(Pipeline {
        when,
        scope: s.scope.clone(),
        discovered,
        rows,
        dead,
        recommend: rec,
    })
}

pub fn discover(
    http: &Http,
    p: Provider,
    auth: &(String, String),
    scope: &str,
) -> Result<Vec<String>, AppError> {
    let json = http.get_json(&format!("{}/models", p.base_url()), auth)?;
    let mut ids: Vec<String> = json
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|o| o.get("id").and_then(|i| i.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    Ok(filter_models(p, ids, scope))
}

pub fn probe_many(
    http: &Http,
    p: Provider,
    auth: &(String, String),
    models: &[String],
    concurrency: usize,
) -> Vec<ProbeRow> {
    let workers = concurrency.max(1).min(models.len().max(1));
    let out = Mutex::new(Vec::<ProbeRow>::new());
    std::thread::scope(|scope| {
        for w in 0..workers {
            let out = &out;
            scope.spawn(move || {
                for m in models.iter().skip(w).step_by(workers) {
                    let row = match http.probe_model(p.base_url(), auth, m) {
                        ProbeOutcome::Ok { ms } => ProbeRow {
                            provider: p.name().into(),
                            model: m.clone(),
                            live: true,
                            ms: Some(ms),
                            reason: None,
                        },
                        ProbeOutcome::Http { code, ms } => ProbeRow {
                            provider: p.name().into(),
                            model: m.clone(),
                            live: false,
                            ms: Some(ms),
                            reason: Some(format!("HTTP {code}")),
                        },
                        ProbeOutcome::Timeout => ProbeRow {
                            provider: p.name().into(),
                            model: m.clone(),
                            live: false,
                            ms: None,
                            reason: Some("timeout".into()),
                        },
                        ProbeOutcome::NetErr(e) => ProbeRow {
                            provider: p.name().into(),
                            model: m.clone(),
                            live: false,
                            ms: None,
                            reason: Some(e),
                        },
                    };
                    out.lock().unwrap().push(row);
                }
            });
        }
    });
    let mut v = out.into_inner().unwrap();
    v.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.model.cmp(&b.model)));
    v
}

pub fn bench_many(
    http: &Http,
    auths: &[(Provider, (String, String))],
    jobs: &[(Provider, String)],
    s: &Settings,
    ctx: &Context,
) -> Vec<Row> {
    if jobs.is_empty() {
        return Vec::new();
    }
    let workers = s.concurrency.max(1).min(jobs.len());
    let out = Mutex::new(Vec::<Row>::new());
    let done = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for w in 0..workers {
            let out = &out;
            let done = &done;
            scope.spawn(move || {
                for job in jobs.iter().skip(w).step_by(workers) {
                    let (p, m) = job;
                    let auth = auths
                        .iter()
                        .find(|(pp, _)| pp == p)
                        .map(|(_, a)| a)
                        .expect("bench: auth 缺失");
                    let row = match run_round(http, *p, auth, m, &s.prompt, s.max_tokens, s.reps) {
                        Ok(r) => r,
                        Err(e) => Row::failed(*p, m, e.to_string()),
                    };
                    out.lock().unwrap().push(row);
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    ctx.tick(
                        n as u64,
                        Some(jobs.len() as u64),
                        format!("[{}] {}", p.name(), m),
                    );
                }
            });
        }
    });
    let mut v = out.into_inner().unwrap();
    v.sort_by(|a, b| {
        b.tokens_per_sec
            .unwrap_or(0.0)
            .partial_cmp(&a.tokens_per_sec.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    v
}

/// 对单个模型重复 `reps` 次并取中位数；正文取内容最长的一次（信息量最大）。
pub fn run_round(
    http: &Http,
    provider: Provider,
    auth: &(String, String),
    model: &str,
    prompt: &str,
    max_tokens: u32,
    reps: u32,
) -> Result<Row, AppError> {
    let mut totals: Vec<u64> = Vec::new();
    let mut best: Option<StreamAcc> = None;
    let mut last_err: Option<AppError> = None;
    for _ in 0..reps.max(1) {
        match http.chat_stream(provider.base_url(), auth, model, prompt, max_tokens) {
            Ok(acc) => {
                totals.push(acc.total_ms);
                if best
                    .as_ref()
                    .is_none_or(|b| acc.content.len() > b.content.len())
                {
                    best = Some(acc);
                }
            }
            Err(e) => last_err = Some(e),
        }
    }
    let Some(acc) = best else {
        return Ok(Row::failed(
            provider,
            model,
            last_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| "无任何有效响应".into()),
        ));
    };
    let mut row = Row::from_acc(provider, model, &acc);
    if let Some(median_ms) = median(&mut totals) {
        row.total_ms = Some(median_ms);
        row.tokens_per_sec = row.tokens.map(|t| t as f64 * 1000.0 / median_ms as f64);
    }
    Ok(row)
}

fn auth_for(
    auths: &[(Provider, (String, String))],
    p: Provider,
) -> Result<&(String, String), AppError> {
    auths
        .iter()
        .find(|(pp, _)| *pp == p)
        .map(|(_, a)| a)
        .ok_or_else(|| AppError::Runtime(format!("内部错误：缺少 {} 凭据", p.name())))
}

// ── UTC 时间格式（不引 chrono） ───────────────────────────

pub fn now_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_utc(secs)
}

/// `YYYY-MM-DD HH:MM:SSZ`。
pub fn format_utc(secs: u64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}Z")
}

/// Howard Hinnant 的 civil_from_days 算法。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sse() -> String {
        r#"data: {"choices":[{"delta":{"reasoning_content":"先想想。"},"index":0}]}
data: {"choices":[{"delta":{"content":"春天"},"index":0}]}
data: {"choices":[{"delta":{"content":"西湖"},"index":0}],"usage":{"prompt_tokens":10,"completion_tokens":8}}
data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}]}
data: [DONE]
"#
        .to_string()
    }

    #[test]
    fn ingest_builds_content_reasoning_and_usage() {
        let mut acc = StreamAcc::default();
        let mut done = false;
        for (i, line) in sse().lines().enumerate() {
            done = ingest(line, (i as u64 + 1) * 2, &mut acc);
        }
        assert!(done);
        assert_eq!(acc.content, "春天西湖");
        assert_eq!(acc.reasoning, "先想想。");
        assert_eq!(acc.ttft_ms, Some(4)); // 第一段 content 在第 2 行
        assert_eq!(acc.completion_tokens, Some(8));
        assert_eq!(acc.prompt_tokens, Some(10));
        assert_eq!(acc.finish.as_deref(), Some("stop"));
    }

    #[test]
    fn ingest_ignores_garbage_and_keepalives() {
        let mut acc = StreamAcc::default();
        assert!(!ingest("", 0, &mut acc));
        assert!(!ingest("event: ping", 0, &mut acc));
        assert!(!ingest("data: not-json", 0, &mut acc));
        assert!(acc.content.is_empty());
        assert!(ingest("data: [DONE]", 0, &mut acc));
    }

    #[test]
    fn chinese_and_token_estimates() {
        assert!(has_chinese("你好"));
        assert!(!has_chinese("hello"));
        assert_eq!(estimate_tokens("你好abc"), 2 + 1); // 2 CJK + 3/4→1
        assert_eq!(estimate_tokens("一"), 1);
    }

    #[test]
    fn median_handles_even_and_odd() {
        assert_eq!(median(&mut [3, 1, 2]), Some(2));
        assert_eq!(median(&mut [10, 2, 4, 8]), Some(6));
        assert_eq!(median(&mut []), None);
    }

    #[test]
    fn recommend_picks_fastest_chinese_answering_model() {
        let mut a = Row::failed(Provider::Nim, "slow", "x".into());
        a.ok = true;
        a.answered = true;
        a.chinese = true;
        a.tokens_per_sec = Some(10.0);
        let mut b = Row::failed(Provider::OpenRouter, "fast", "x".into());
        b.ok = true;
        b.answered = true;
        b.chinese = true;
        b.tokens_per_sec = Some(80.0);
        let mut c = Row::failed(Provider::Nim, "no-cn", "x".into());
        c.ok = true;
        c.answered = true;
        c.chinese = false;
        c.tokens_per_sec = Some(500.0);
        let rec = recommend(&[a, b, c]).unwrap();
        assert_eq!(rec.model, "fast");
        assert_eq!(rec.provider, "openrouter");
    }

    #[test]
    fn no_chinese_answer_means_no_recommendation() {
        let mut r = Row::failed(Provider::Nim, "m", "x".into());
        r.answered = true;
        r.chinese = false;
        r.tokens_per_sec = Some(100.0);
        assert!(recommend(&[r]).is_none());
    }

    #[test]
    fn format_utc_epoch_and_known_dates() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(format_utc(86_400), "1970-01-02 00:00:00Z");
        // 2026-10-10 00:00:00Z（epoch → 2026-01-01 共 20_454 天，再加 282 天）
        assert_eq!(format_utc(20_736 * 86_400), "2026-10-10 00:00:00Z");
    }
}
