//! Автообновление: свежий релиз берётся с GitHub и подменяет запущенный exe.

use std::{io::Read, path::PathBuf};

use sha2::{Digest, Sha256};

const REPO: &str = "Kamaliev/cs2cinema";

/// Версия релиза, из которого собран exe (CI задаёт `CS2CINEMA_VERSION=v1.2.3`); в dev-сборке — `None`.
pub fn current() -> Option<&'static str> {
    option_env!("CS2CINEMA_VERSION").filter(|v| !v.is_empty())
}

#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub zip_url: String,
    pub sha_url: Option<String>,
}

fn parse_ver(v: &str) -> Vec<u32> {
    v.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

pub fn newer(tag: &str, current: &str) -> bool {
    parse_ver(tag) > parse_ver(current)
}

fn agent() -> ureq::Agent {
    let cfg = ureq::Agent::config_builder()
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .user_agent("cs2cinema-updater")
        .build();
    ureq::Agent::new_with_config(cfg)
}

/// Свежий релиз, если он новее запущенного.
pub fn check() -> Result<Option<Release>, String> {
    let Some(cur) = current() else { return Ok(None) };
    let v: serde_json::Value = agent()
        .get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("в ответе GitHub нет tag_name")?.to_owned();
    if !newer(&tag, cur) {
        return Ok(None);
    }
    let asset = |suffix: &str| {
        v["assets"].as_array()?.iter().find_map(|a| {
            let n = a["name"].as_str()?;
            n.ends_with(suffix).then(|| a["browser_download_url"].as_str().map(str::to_owned))?
        })
    };
    let zip_url = asset("windows-x64.zip").ok_or("в релизе нет cs2cinema-windows-x64.zip")?;
    Ok(Some(Release { tag, zip_url, sha_url: asset("windows-x64.zip.sha256") }))
}

/// Скачивает релиз, проверяет sha256 и подменяет текущий exe. Возвращает путь к новому exe.
pub fn install(r: &Release) -> Result<PathBuf, String> {
    let agent = agent();
    let mut bytes = Vec::new();
    agent.get(&r.zip_url).call().map_err(|e| e.to_string())?.body_mut().as_reader().read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if let Some(u) = &r.sha_url {
        let text = agent.get(u).call().map_err(|e| e.to_string())?.body_mut().read_to_string().map_err(|e| e.to_string())?;
        let want = text.split_whitespace().next().unwrap_or("").to_lowercase();
        let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        if want != got {
            return Err("контрольная сумма обновления не совпала — установка отменена".into());
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("нет папки у exe")?.to_path_buf();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut replaced = None;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = f.enclosed_name().and_then(|p| p.file_name().map(|n| n.to_owned())) else { continue };
        let name = name.to_string_lossy().into_owned();
        if !name.to_lowercase().ends_with(".exe") {
            continue;
        }
        let mut data = Vec::new();
        f.read_to_end(&mut data).map_err(|e| e.to_string())?;
        let target = dir.join(&name);
        // запущенный exe нельзя перезаписать, но можно переименовать
        if target.exists() {
            let old = target.with_extension("exe.old");
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&target, &old).map_err(|e| format!("{}: {e}", target.display()))?;
        }
        std::fs::write(&target, data).map_err(|e| format!("{}: {e}", target.display()))?;
        if target == exe {
            replaced = Some(target);
        }
    }
    replaced.ok_or_else(|| "в архиве нет cs2cinema.exe".into())
}
