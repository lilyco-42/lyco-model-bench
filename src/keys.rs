//! API key 解析与查找。
//!
//! 密钥来源，按优先级：运行时环境变量 > keys 文件（默认
//! `~/.config/lyco-llm/keys.env`，可用 `$LYCO_KEYS_FILE` 覆盖，或 `--keys <path>`）。
//! 任何路径都**不打印密钥值**——日志里只出现变量名与来源。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lilyco::prelude::AppError;

/// 从 keys.env 文本解析键值对。
///
/// 兼容 `KEY=value`、`export KEY=value`、值两侧的单/双引号、`#` 注释与空行。
/// 空键或空值一律丢弃（把「占位空行」当没写）。
pub fn parse_keys(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // 去掉可选的 `export ` 前缀（仅当其后紧跟空白，避免误伤名字里含 export 的键）
        let line = match line.strip_prefix("export") {
            Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim_start(),
            _ => line,
        };
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim().to_string();
        let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
        if !key.is_empty() && !val.is_empty() {
            map.insert(key, val);
        }
    }
    map
}

/// 密钥仓库：一次性装载后按 env 名取用。
pub struct KeyStore {
    file: Option<PathBuf>,
    parsed: HashMap<String, String>,
}

impl KeyStore {
    /// 装载密钥。`path` 为空时探测默认位置（`$LYCO_KEYS_FILE` → `~/.config/lyco-llm/keys.env`）。
    /// 文件不存在不算错误——环境变量可能已经提供全部密钥，取用时报错更贴近现场。
    pub fn load(path: Option<&Path>) -> Result<Self, AppError> {
        let file = match path {
            Some(p) if !p.as_os_str().is_empty() => Some(p.to_path_buf()),
            _ => default_path(),
        };
        let mut parsed = HashMap::new();
        if let Some(f) = &file {
            if f.exists() {
                let text = std::fs::read_to_string(f).map_err(|e| {
                    AppError::Io(std::io::Error::new(
                        e.kind(),
                        format!("读 {} 失败: {e}", f.display()),
                    ))
                })?;
                parsed = parse_keys(&text);
            }
        }
        Ok(Self { file, parsed })
    }

    /// 取一个密钥：环境变量优先，其次 keys 文件。缺失时报错，错误里只说来源与变量名，不泄露值。
    pub fn get(&self, env_name: &str) -> Result<String, AppError> {
        if let Ok(v) = std::env::var(env_name) {
            let v = v.trim();
            if !v.is_empty() {
                return Ok(v.to_string());
            }
        }
        if let Some(v) = self.parsed.get(env_name) {
            if !v.is_empty() {
                return Ok(v.clone());
            }
        }
        let where_ = self
            .file
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<未找到默认位置>".to_string());
        Err(AppError::Runtime(format!(
            "缺少 API key `{env_name}`：请设该环境变量，或写入 keys 文件（{where_}）"
        )))
    }
}

fn default_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LYCO_KEYS_FILE") {
        if !p.trim().is_empty() {
            return Some(PathBuf::from(p.trim()));
        }
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| {
            PathBuf::from(home)
                .join(".config")
                .join("lyco-llm")
                .join("keys.env")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_export_quoted_and_comments() {
        let text = "\
# 注释行
NIM_API_KEY=nvapi-abc123

export OPENROUTER_API_KEY=\"sk-or-xyz\"
QUOTED_SINGLE='single'
EMPTY=
NOEQUALS
   PADDED   =   spaced   
";
        let m = parse_keys(text);
        assert_eq!(m.get("NIM_API_KEY").unwrap(), "nvapi-abc123");
        assert_eq!(m.get("OPENROUTER_API_KEY").unwrap(), "sk-or-xyz");
        assert_eq!(m.get("QUOTED_SINGLE").unwrap(), "single");
        assert_eq!(m.get("PADDED").unwrap(), "spaced");
        // 空值 / 无等号行被丢弃
        assert!(!m.contains_key("EMPTY"));
        assert!(!m.contains_key("NOEQUALS"));
    }

    #[test]
    fn env_var_wins_over_file() {
        let mut parsed = HashMap::new();
        parsed.insert("SOME_TEST_KEY".to_string(), "from-file".to_string());
        let store = KeyStore { file: None, parsed };
        std::env::set_var("SOME_TEST_KEY", "from-env");
        assert_eq!(store.get("SOME_TEST_KEY").unwrap(), "from-env");
        std::env::remove_var("SOME_TEST_KEY");
        assert_eq!(store.get("SOME_TEST_KEY").unwrap(), "from-file");
    }

    #[test]
    fn missing_key_is_an_error_naming_the_variable_only() {
        let store = KeyStore {
            file: None,
            parsed: HashMap::new(),
        };
        let err = store.get("A_VERY_MISSING_KEY").unwrap_err().to_string();
        assert!(err.contains("A_VERY_MISSING_KEY"), "{err}");
    }
}
