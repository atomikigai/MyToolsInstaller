use anyhow::Result;
use std::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distro {
    Arch,
    Other,
}

pub fn distro() -> Distro {
    let Ok(content) = fs::read_to_string("/etc/os-release") else {
        return Distro::Other;
    };
    let ids: Vec<String> = content
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            if key == "ID" || key == "ID_LIKE" {
                Some(value.trim_matches('"').to_string())
            } else {
                None
            }
        })
        .flat_map(|v| {
            v.split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();

    if ids.iter().any(|id| id == "arch" || id == "cachyos") {
        Distro::Arch
    } else {
        Distro::Other
    }
}

pub fn home() -> Result<std::path::PathBuf> {
    Ok(std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("$HOME not set"))?)
}
