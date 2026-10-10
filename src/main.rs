//! lyco-model-bench —— 每日模型发现 + token 测速。
//!
//! NIM / OpenRouter 的模型供给每天都在变：列表里有但一调就卡死的、收费的、换代的……
//! 本工具每天自动：**发现 → 探测（剔卡死）→ 固定中文提示实测 token 速度 → 出带推荐的报告**。
//!
//! 同一个二进制四端可用（lilyco 一个 struct 出 CLI / TUI / Web / MCP）：
//! ```bash
//! lyco-model-bench discover                      # 列出模型
//! lyco-model-bench probe                         # 探测存活
//! lyco-model-bench bench --provider both         # 全流程测速
//! lyco-model-bench daily                         # 每日任务：测完落盘 results/
//! lyco-model-bench report                        # 把结果渲染成 Markdown
//! lyco-model-bench --mcp                         # MCP 服务器（AI 直接调）
//! lyco-model-bench --tui                         # TUI 命令选择页
//! ```
//!
//! 密钥：环境变量 `NIM_API_KEY` / `OPENROUTER_API_KEY` 优先，兜底
//! `~/.config/lyco-llm/keys.env`（`export KEY=value` 一行一个）。工具只读密钥，
//! 任何日志都不打印密钥值。

mod bench;
mod cmd;
mod http;
mod keys;
mod provider;
mod report;

use lilyco::prelude::*;

/// 构建整个域的注册表（多命令：discover / probe / bench / daily / report）。
pub fn build_registry() -> Registry {
    let mut registry = Registry::new();
    registry
        .register(RegisteredCommand::from_app::<cmd::Discover>())
        .unwrap();
    registry
        .register(RegisteredCommand::from_app::<cmd::Probe>())
        .unwrap();
    registry
        .register(RegisteredCommand::from_app::<cmd::Bench>())
        .unwrap();
    registry
        .register(RegisteredCommand::from_app::<cmd::Daily>())
        .unwrap();
    registry
        .register(RegisteredCommand::from_app::<cmd::Report>())
        .unwrap();
    registry
}

fn main() {
    let registry = build_registry();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--mcp") {
        lilyco::serve_mcp(registry);
    } else if args.iter().any(|a| a == "--tui") {
        lilyco::run_tui_registry("lyco-model-bench", registry);
    } else {
        lilyco::run_cli_registry("lyco-model-bench", registry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        let mut v: Vec<String> = build_registry()
            .iter()
            .map(|c| c.schema.name.clone())
            .collect();
        v.sort();
        v
    }

    fn schema_of(name: &str) -> CommandSchema {
        build_registry().get(name).expect("命令存在").schema.clone()
    }

    /// 命令表是四端共同的契约面：改名 = 破坏 agent 侧的调用，断言住。
    #[test]
    fn registry_lists_all_commands() {
        assert_eq!(
            names(),
            vec!["bench", "daily", "discover", "probe", "report"]
        );
    }

    /// 每个命令的 schema 都携带关键参数与缺省值（schema 是 CLI/MCP 对外契约）。
    #[test]
    fn command_schemas_carry_defaults_and_no_required_args() {
        let bench = schema_of("bench");
        let reps = bench.args.iter().find(|a| a.name == "reps").unwrap();
        assert_eq!(reps.default, Some(serde_json::json!(3)));
        assert!(
            bench.args.iter().any(|a| a.name == "max-tokens"),
            "bench 应带 max-tokens"
        );
        assert!(
            schema_of("daily").args.iter().any(|a| a.name == "out"),
            "daily 应带 out 参数"
        );
        assert!(schema_of("discover")
            .args
            .iter()
            .any(|a| a.name == "timeout"));
        // 全部参数可选：CLI / MCP 缺参时由命令自己落缺省
        for n in names() {
            assert!(
                schema_of(&n).args.iter().all(|a| !a.required),
                "{n} 不应有必填参数"
            );
        }
    }

    #[test]
    fn duplicate_command_names_are_rejected() {
        let mut reg = Registry::new();
        reg.register(RegisteredCommand::from_app::<cmd::Discover>())
            .unwrap();
        assert!(
            reg.register(RegisteredCommand::from_app::<cmd::Discover>())
                .is_err(),
            "重名必须报错"
        );
    }
}
