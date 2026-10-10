//! Provider 配置：NIM 与 OpenRouter 的端点、密钥变量名与模型筛选。

use lilyco::prelude::AppError;

use crate::keys::KeyStore;

/// NVIDIA NIM（integrate.api.nvidia.com）。
pub const NIM_BASE: &str = "https://integrate.api.nvidia.com/v1";
/// OpenRouter（OpenAI 兼容）。
pub const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Nim,
    OpenRouter,
}

pub const ALL_PROVIDERS: [Provider; 2] = [Provider::Nim, Provider::OpenRouter];

impl Provider {
    /// 报告与行里的短名。
    pub fn name(self) -> &'static str {
        match self {
            Provider::Nim => "nim",
            Provider::OpenRouter => "openrouter",
        }
    }

    /// OpenAI 兼容 API 的根（后面拼 `/chat/completions`、`/models`）。
    pub fn base_url(self) -> &'static str {
        match self {
            Provider::Nim => NIM_BASE,
            Provider::OpenRouter => OPENROUTER_BASE,
        }
    }

    /// 承载密钥的环境变量名。
    pub fn key_env(self) -> &'static str {
        match self {
            Provider::Nim => "NIM_API_KEY",
            Provider::OpenRouter => "OPENROUTER_API_KEY",
        }
    }

    /// 解析出 `Authorization` 头（值不在任何日志里出现）。
    pub fn auth(self, keys: &KeyStore) -> Result<(String, String), AppError> {
        let key = keys.get(self.key_env())?;
        Ok(("Authorization".to_string(), format!("Bearer {key}")))
    }
}

/// 解析 `--provider`。空串按「两者」。
pub fn parse_providers(s: &str) -> Result<Vec<Provider>, AppError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "" | "both" | "all" => Ok(ALL_PROVIDERS.to_vec()),
        "nim" | "nvidia" => Ok(vec![Provider::Nim]),
        "openrouter" | "or" => Ok(vec![Provider::OpenRouter]),
        other => Err(AppError::InvalidArg(format!(
            "未知 provider `{other}`（可选: nim / openrouter / both）"
        ))),
    }
}

/// 按 scope 过滤模型 id。scope: `free`（默认）/ `all`。
///
/// NIM 账号下返回的模型都可直接调，故 scope 不改变 NIM 的结果；OpenRouter 存在大量
/// 付费模型，`free` 只保留 id 以 `:free` 结尾的。
pub fn filter_models(p: Provider, ids: Vec<String>, scope: &str) -> Vec<String> {
    match p {
        Provider::Nim => ids,
        Provider::OpenRouter => match scope.trim() {
            "all" => ids,
            _ => ids.into_iter().filter(|id| id.ends_with(":free")).collect(),
        },
    }
}

/// 显式 `--model` 未指定 provider 时，按 id 后缀推断它属于哪个 provider。
pub fn provider_for_model(id: &str) -> Provider {
    if id.ends_with(":free") {
        Provider::OpenRouter
    } else {
        Provider::Nim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_parsing_defaults_to_both_and_rejects_junk() {
        assert_eq!(
            parse_providers("").unwrap(),
            vec![Provider::Nim, Provider::OpenRouter]
        );
        assert_eq!(parse_providers("both").unwrap().len(), 2);
        assert_eq!(parse_providers("NIM").unwrap(), vec![Provider::Nim]);
        assert_eq!(
            parse_providers("openrouter").unwrap(),
            vec![Provider::OpenRouter]
        );
        assert!(parse_providers("anthropic").is_err());
    }

    #[test]
    fn nim_scope_is_a_noop_but_openrouter_free_filters() {
        let ids = vec![
            "meta/llama-3.2-11b-vision-instruct".to_string(),
            "nvidia/nemotron-3-super-120b-a12b:free".to_string(),
            "vendor/paid-big-model".to_string(),
        ];
        assert_eq!(filter_models(Provider::Nim, ids.clone(), "free").len(), 3);
        let free = filter_models(Provider::OpenRouter, ids.clone(), "free");
        assert_eq!(
            free,
            vec!["nvidia/nemotron-3-super-120b-a12b:free".to_string()]
        );
        assert_eq!(filter_models(Provider::OpenRouter, ids, "all").len(), 3);
    }

    #[test]
    fn model_suffix_decides_the_provider_when_unspecified() {
        assert_eq!(provider_for_model("nvidia/x"), Provider::Nim);
        assert_eq!(provider_for_model("nvidia/x:free"), Provider::OpenRouter);
    }
}
