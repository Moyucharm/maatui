use crate::storage::{maa_data_dir, maa_hot_update_dir};
use std::path::PathBuf;

pub(super) fn tile_pos_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(data_dir) = maa_data_dir() {
        dirs.push(data_dir.join("resource/Arknights-Tile-Pos"));
        dirs.push(data_dir.join("MaaResource/resource/Arknights-Tile-Pos"));
    }
    if let Ok(hot_update_dir) = maa_hot_update_dir() {
        dirs.push(hot_update_dir.join("resource/Arknights-Tile-Pos"));
    }
    let mut unique = Vec::new();
    for dir in dirs.into_iter().filter(|path| path.is_dir()) {
        if !unique.contains(&dir) {
            unique.push(dir);
        }
    }
    unique
}
