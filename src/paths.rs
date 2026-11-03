//! 本机数据目录。

use std::path::PathBuf;

/// anime-organizer 的本机数据目录。
///
/// Windows 使用 `%LOCALAPPDATA%\anime-organizer`，其他系统使用
/// `$XDG_DATA_HOME/anime-organizer` 或 `~/.local/share/anime-organizer`。
pub(crate) fn data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        let root = std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        return root.join("anime-organizer");
    }

    #[cfg(not(windows))]
    {
        let root = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".local").join("share"))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        root.join("anime-organizer")
    }
}
