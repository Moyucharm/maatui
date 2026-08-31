use anyhow::{Context, Result};
use std::path::PathBuf;

pub(crate) fn maa_config_dir() -> Result<PathBuf> {
    resolve_maa_config_dir(
        std::env::var_os("MAA_CONFIG_DIR"),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

pub(crate) fn resolve_maa_config_dir(
    maa_config_dir: Option<std::ffi::OsString>,
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf> {
    if let Some(dir) = maa_config_dir {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = xdg_config_home {
        return Ok(PathBuf::from(dir).join("maa"));
    }
    let home = home.context("无法确定 HOME，且未设置 MAA_CONFIG_DIR/XDG_CONFIG_HOME")?;
    Ok(PathBuf::from(home).join(".config/maa"))
}

pub(crate) fn maa_data_dir() -> Result<PathBuf> {
    resolve_maa_data_dir(
        std::env::var_os("MAA_DATA_DIR"),
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
    )
}

pub(crate) fn maa_log_dir() -> Result<PathBuf> {
    Ok(resolve_maa_state_dir(
        std::env::var_os("MAA_STATE_DIR"),
        std::env::var_os("XDG_STATE_HOME"),
        std::env::var_os("HOME"),
    )?
    .join("debug"))
}

pub(crate) fn resolve_maa_state_dir(
    maa_state_dir: Option<std::ffi::OsString>,
    xdg_state_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf> {
    if let Some(dir) = maa_state_dir {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = xdg_state_home {
        return Ok(PathBuf::from(dir).join("maa"));
    }
    let home = home.context("无法确定 HOME，且未设置 MAA_STATE_DIR/XDG_STATE_HOME")?;
    Ok(PathBuf::from(home).join(".local/state/maa"))
}

pub(crate) fn resolve_maa_data_dir(
    maa_data_dir: Option<std::ffi::OsString>,
    xdg_data_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf> {
    if let Some(dir) = maa_data_dir {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = xdg_data_home {
        return Ok(PathBuf::from(dir).join("maa"));
    }
    let home = home.context("无法确定 HOME，且未设置 MAA_DATA_DIR/XDG_DATA_HOME")?;
    Ok(PathBuf::from(home).join(".local/share/maa"))
}

pub(crate) fn maa_hot_update_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("MAA_HOT_UPDATE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    Ok(maa_data_dir()?.join("MaaResource"))
}

pub(crate) fn maatui_cache_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(dir).join("maatui"));
    }
    let home = std::env::var_os("HOME").context("无法确定 HOME，且未设置 XDG_CACHE_HOME")?;
    Ok(PathBuf::from(home).join(".cache/maatui"))
}
