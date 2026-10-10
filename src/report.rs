//! 把 [`Pipeline`] 渲染成人类可读的 Markdown 报告（`results/latest.md`）。

use crate::bench::{Pipeline, Row};

/// 渲染整份报告。稳定、可 diff——每日落在同一路径，变更一目了然。
pub fn render_md(p: &Pipeline) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Model speed report — {}\n\n", p.when));

    let scope = if p.scope.is_empty() {
        "free"
    } else {
        p.scope.as_str()
    };
    out.push_str(&format!("scope: `{scope}`\n\n"));

    match &p.recommend {
        Some(r) => out.push_str(&format!(
            "**RECOMMEND:** `{} / {}` — {} tok/s — 中文问答首选\n\n",
            r.provider,
            r.model,
            fmt_f(r.tokens_per_sec, 1)
        )),
        None => out.push_str("**RECOMMEND:** (none — 没有能中文回答的模型)\n\n"),
    }

    if p.rows.is_empty() {
        out.push_str("_（本次没有测到任何模型）_\n");
    } else {
        out.push_str("| provider | model | ok | answered | chinese | tok/s | ttft(ms) | total(ms) | tokens | answer | reason_chars |\n");
        out.push_str("|---|---|---|---|---|---|---|---|---|---|---|\n");
        for r in &p.rows {
            out.push_str(&row_md(r));
            out.push('\n');
        }
    }

    if !p.dead.is_empty() {
        out.push_str("\n## 探测未通过（卡死 / 下线 / 无权限）\n\n");
        for d in &p.dead {
            out.push_str(&format!(
                "- `{} / {}` — {}\n",
                d.provider,
                d.model,
                d.reason.as_deref().unwrap_or("?")
            ));
        }
    }
    out
}

fn row_md(r: &Row) -> String {
    format!(
        "| {} | `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
        r.provider,
        r.model,
        yesno(r.ok),
        yesno(r.answered),
        yesno(r.chinese),
        fmt_f(r.tokens_per_sec, 1),
        fmt_u(r.ttft_ms),
        fmt_u(r.total_ms),
        fmt_tokens(r),
        esc(&r.answer_preview),
        r.reasoning_chars,
    )
}

fn yesno(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

fn fmt_f(v: Option<f64>, dec: usize) -> String {
    match v {
        Some(x) => format!("{x:.dec$}"),
        None => "–".to_string(),
    }
}

fn fmt_u(v: Option<u64>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "–".to_string())
}

fn fmt_tokens(r: &Row) -> String {
    match r.tokens {
        Some(t) if r.tokens_estimated => format!("~{t}"),
        Some(t) => t.to_string(),
        None => "–".to_string(),
    }
}

fn esc(s: &str) -> String {
    s.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::{recommend, StreamAcc};
    use crate::provider::Provider;

    fn sample_pipeline() -> Pipeline {
        let acc = StreamAcc {
            content: "西湖春色，柳浪闻莺。".into(),
            ttft_ms: Some(180),
            total_ms: 900,
            completion_tokens: Some(12),
            ..Default::default()
        };
        let row = Row::from_acc(Provider::Nim, "nvidia/demo", &acc);
        let rows = vec![row];
        Pipeline {
            when: "2026-10-10 00:00:00Z".into(),
            scope: String::new(),
            discovered: vec![("nim".into(), 1)],
            recommend: recommend(&rows),
            rows,
            dead: vec![],
        }
    }

    #[test]
    fn renders_recommendation_table_and_scope() {
        let md = render_md(&sample_pipeline());
        assert!(md.contains("RECOMMEND"), "{md}");
        assert!(md.contains("nvidia/demo"), "{md}");
        assert!(md.contains("scope: `free`"), "{md}");
        assert!(md.contains("| provider | model |"), "{md}");
        // 该行应判为能答 + 中文（取表格行而不是 RECOMMEND 行）
        let line = md
            .lines()
            .find(|l| l.starts_with("| ") && l.contains("nvidia/demo"))
            .unwrap();
        assert!(line.contains("| yes | yes | yes |"), "{line}");
    }

    #[test]
    fn empty_pipeline_says_none_without_panicking() {
        let p = Pipeline {
            when: "t".into(),
            scope: "all".into(),
            discovered: vec![],
            rows: vec![],
            dead: vec![],
            recommend: None,
        };
        let md = render_md(&p);
        assert!(md.contains("RECOMMEND:** (none"), "{md}");
        assert!(md.contains("scope: `all`"), "{md}");
    }
}
