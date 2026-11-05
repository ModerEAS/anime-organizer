//! 元数据数据源设置。
//!
//! 存放在本机数据目录的 `metadata.json`（与 `daemon.db` / `update.json` 同一层），
//! CLI 与 daemon 共用；WebUI 可以直接读写。
//!
//! ```json
//! { "auto_download_alias_db": true }
//! ```

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

fn default_true() -> bool {
    true
}

/// 元数据相关的本地设置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct MetadataSettings {
    /// 是否自动下载周更的全量别名库 `bangumi.db`（约 120 MB）。
    ///
    /// 关闭后只用本地已有的 `bangumi.db`；本地没有时才回退到 91 部作品的
    /// AnimeAtlas 小库。适合流量受限或只想离线使用的场景。
    #[serde(default = "default_true")]
    pub(crate) auto_download_alias_db: bool,
}

impl Default for MetadataSettings {
    fn default() -> Self {
        Self {
            auto_download_alias_db: true,
        }
    }
}

impl MetadataSettings {
    pub(crate) fn path() -> PathBuf {
        crate::paths::data_dir().join("metadata.json")
    }

    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path).map_err(|err| format!("读取元数据配置失败：{err}"))?;
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(&text).map_err(|err| format!("元数据配置无法解析：{err}"))
    }

    /// 仅 daemon（WebUI 设置接口）与测试会写回文件。
    #[cfg(any(feature = "daemon", test))]
    pub(crate) fn save(path: &Path, settings: &Self) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("创建元数据配置目录失败：{err}"))?;
        }
        let body = serde_json::to_vec_pretty(settings)
            .map_err(|err| format!("序列化元数据配置失败：{err}"))?;
        fs::write(path, body).map_err(|err| format!("写入元数据配置失败：{err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_downloading_the_full_alias_db() {
        assert!(MetadataSettings::default().auto_download_alias_db);
    }

    #[test]
    fn missing_file_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metadata.json");
        assert_eq!(
            MetadataSettings::load(&path).unwrap(),
            MetadataSettings::default()
        );
    }

    #[test]
    fn round_trips_and_accepts_partial_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metadata.json");
        let settings = MetadataSettings {
            auto_download_alias_db: false,
        };
        MetadataSettings::save(&path, &settings).unwrap();
        assert_eq!(MetadataSettings::load(&path).unwrap(), settings);

        // 旧版本/手写的空对象应当回落到默认值
        fs::write(&path, "{}").unwrap();
        assert!(
            MetadataSettings::load(&path)
                .unwrap()
                .auto_download_alias_db
        );
    }
}
