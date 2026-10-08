//! Скачивание демки матча FACEIT по ссылке.
//!
//! Нужен ключ FACEIT Data API (`FACEIT_API_KEY`) с доступом к Downloads API.

use std::{
    error::Error,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const DATA_API: &str = "https://open.faceit.com/data/v4";
const DOWNLOAD_API: &str = "https://open.faceit.com/download/v2/demos/download";

/// `1-3e7db9e3-8e92-4761-a1db-3729fb7de11c` из ссылки вида
/// `https://www.faceit.com/en/cs2/room/1-3e7d.../scoreboard` (или из самого id).
pub fn parse_match_id(input: &str) -> Option<String> {
    input
        .trim()
        .split(|c: char| matches!(c, '/' | '?' | '#' | ' '))
        .find(|part| is_match_id(part))
        .map(str::to_owned)
}

fn is_match_id(s: &str) -> bool {
    // <префикс-цифры>-<uuid>
    let Some((prefix, uuid)) = s.split_once('-') else { return false };
    let groups: Vec<&str> = uuid.split('-').collect();
    !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_digit())
        && groups.len() == 5
        && groups.iter().zip([8, 4, 4, 4, 12]).all(|(g, n)| g.len() == n && g.chars().all(|c| c.is_ascii_hexdigit()))
}

pub struct Client {
    key: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(key: impl Into<String>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            // системное хранилище сертификатов: работает и за корпоративными прокси
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
            .build()
            .into();
        Self { key: key.into(), agent }
    }

    pub fn from_env() -> Result<Self> {
        let key = std::env::var("FACEIT_API_KEY")
            .map_err(|_| "задайте ключ FACEIT Data API в переменной окружения FACEIT_API_KEY")?;
        Ok(Self::new(key))
    }

    /// Ссылки на демки матча (в bo3 их несколько — по одной на карту).
    pub fn demo_urls(&self, match_id: &str) -> Result<Vec<String>> {
        let mut resp = self
            .agent
            .get(&format!("{DATA_API}/matches/{match_id}"))
            .header("Authorization", &format!("Bearer {}", self.key))
            .call()?;
        check(resp.status().as_u16(), "матч")?;
        let json: serde_json::Value = resp.body_mut().read_json()?;
        let urls: Vec<String> = json["demo_url"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        if urls.is_empty() {
            return Err("у матча нет демки (ещё не доиграли или демка удалена)".into());
        }
        Ok(urls)
    }

    /// Скачивает демку в `dir` (кэшируется по имени файла) и возвращает путь.
    pub fn download(&self, resource_url: &str, dir: &Path) -> Result<PathBuf> {
        let name = resource_url.rsplit('/').next().filter(|n| !n.is_empty()).ok_or("странный demo_url")?;
        fs::create_dir_all(dir)?;
        let path = dir.join(name);
        if path.metadata().is_ok_and(|m| m.len() > 0) {
            return Ok(path);
        }

        let mut resp = self
            .agent
            .post(DOWNLOAD_API)
            .header("Authorization", &format!("Bearer {}", self.key))
            .send_json(serde_json::json!({ "resource_url": resource_url }))?;
        check(resp.status().as_u16(), "ссылка на скачивание (нужен ключ с доступом к Downloads API)")?;
        let json: serde_json::Value = resp.body_mut().read_json()?;
        let url = json["payload"]["download_url"].as_str().ok_or("в ответе нет download_url")?;

        let mut resp = self.agent.get(url).call()?;
        check(resp.status().as_u16(), "файл демки")?;
        let tmp = path.with_extension("part");
        let mut file = fs::File::create(&tmp)?;
        std::io::copy(&mut resp.body_mut().with_config().limit(u64::MAX).reader().by_ref(), &mut file)?;
        fs::rename(&tmp, &path)?;
        Ok(path)
    }
}

fn check(status: u16, what: &str) -> Result<()> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(format!("FACEIT отказал в доступе ({what}): проверьте FACEIT_API_KEY").into()),
        404 => Err(format!("FACEIT: не найдено ({what})").into()),
        s => Err(format!("FACEIT ответил {s} ({what})").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "1-3e7db9e3-8e92-4761-a1db-3729fb7de11c";

    #[test]
    fn parses_room_urls_and_bare_ids() {
        for input in [
            format!("https://www.faceit.com/en/cs2/room/{ID}"),
            format!("https://www.faceit.com/ru/cs2/room/{ID}/scoreboard?x=1"),
            format!("  {ID} "),
        ] {
            assert_eq!(parse_match_id(&input).as_deref(), Some(ID), "{input}");
        }
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_match_id("https://www.faceit.com/en/players/someone"), None);
        assert_eq!(parse_match_id("1-3e7db9e3-8e92-4761-a1db"), None);
    }
}
