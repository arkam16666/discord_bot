//! เก็บการตั้งค่าของแต่ละเซิร์ฟเวอร์ลงไฟล์ JSON (ปิดบอทแล้วค่าไม่หาย)

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};

use crate::Error;

/// การตั้งค่าของเซิร์ฟเวอร์หนึ่ง (เก็บ ID เป็นตัวเลข)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GuildSettings {
    /// ห้องแจ้งเตือนคนเข้า (ไฟล์ตั้งค่าเวอร์ชันเก่าใช้ชื่อ member_log)
    #[serde(alias = "member_log")]
    pub join_log: Option<u64>,
    /// ห้องแจ้งเตือนคนออก
    pub leave_log: Option<u64>,
    /// หมวดหมู่ที่บอทสร้างให้ (มีค่า = ห้องสถิติเป็นของบอท ปิดแล้วจะลบทิ้ง)
    pub stats_category: Option<u64>,
    /// ห้องแสดงจำนวนสมาชิกทั้งหมด
    pub stats_total: Option<u64>,
    /// ห้องแสดงจำนวนคนออนไลน์
    pub stats_online: Option<u64>,
    /// รูปแบบชื่อห้องจำนวนสมาชิก ใช้ {count} แทนตัวเลข
    pub total_name: Option<String>,
    /// รูปแบบชื่อห้องคนออนไลน์ ใช้ {count} แทนตัวเลข
    pub online_name: Option<String>,
}

pub const DEFAULT_TOTAL_NAME: &str = "สมาชิกทั้งหมด: {count}";
pub const DEFAULT_ONLINE_NAME: &str = "ออนไลน์: {count}";

/// ชนิดของการแจ้งเตือนสมาชิก
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogKind {
    Join,
    Leave,
}

impl LogKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Join => "คนเข้า",
            Self::Leave => "คนออก",
        }
    }
}

impl GuildSettings {
    pub fn log(&self, kind: LogKind) -> Option<serenity::ChannelId> {
        let id = match kind {
            LogKind::Join => self.join_log,
            LogKind::Leave => self.leave_log,
        };
        id.map(serenity::ChannelId::new)
    }

    pub fn set_log(&mut self, kind: LogKind, channel: Option<serenity::ChannelId>) {
        let id = channel.map(|c| c.get());
        match kind {
            LogKind::Join => self.join_log = id,
            LogKind::Leave => self.leave_log = id,
        }
    }

    pub fn total_template(&self) -> &str {
        self.total_name.as_deref().unwrap_or(DEFAULT_TOTAL_NAME)
    }

    pub fn online_template(&self) -> &str {
        self.online_name.as_deref().unwrap_or(DEFAULT_ONLINE_NAME)
    }

    pub fn has_stats(&self) -> bool {
        self.stats_total.is_some() || self.stats_online.is_some()
    }
}

pub struct Settings {
    path: PathBuf,
    guilds: RwLock<HashMap<u64, GuildSettings>>,
    /// กันการเขียนไฟล์พร้อมกันหลายครั้ง
    write_lock: tokio::sync::Mutex<()>,
}

impl Settings {
    /// โหลดจากไฟล์ ถ้าไม่มีไฟล์ให้เริ่มใหม่ ถ้าไฟล์เสียให้สำรองไว้แล้วเริ่มใหม่
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let guilds = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<HashMap<u64, GuildSettings>>(&text) {
                Ok(map) => {
                    println!("[ตั้งค่า] โหลดการตั้งค่า {} เซิร์ฟเวอร์", map.len());
                    map
                }
                Err(e) => {
                    let backup = path.with_extension("json.broken");
                    eprintln!(
                        "[คำเตือน] ไฟล์ตั้งค่า {} เสีย ({e}) จะสำรองไว้ที่ {} แล้วเริ่มใหม่",
                        path.display(),
                        backup.display()
                    );
                    if let Err(e) = std::fs::rename(&path, &backup) {
                        eprintln!("[คำเตือน] สำรองไฟล์ตั้งค่าไม่สำเร็จ: {e}");
                    }
                    HashMap::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => {
                eprintln!("[คำเตือน] อ่านไฟล์ตั้งค่า {} ไม่ได้: {e} จะใช้ค่าว่าง", path.display());
                HashMap::new()
            }
        };
        Self {
            path,
            guilds: RwLock::new(guilds),
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn get(&self, guild: serenity::GuildId) -> GuildSettings {
        let map = self.guilds.read().unwrap_or_else(|e| e.into_inner());
        map.get(&guild.get()).cloned().unwrap_or_default()
    }

    /// รายการเซิร์ฟเวอร์ที่เปิดช่องสถิติไว้
    pub fn with_stats(&self) -> Vec<(serenity::GuildId, GuildSettings)> {
        let map = self.guilds.read().unwrap_or_else(|e| e.into_inner());
        map.iter()
            .filter(|(_, s)| s.has_stats())
            .map(|(id, s)| (serenity::GuildId::new(*id), s.clone()))
            .collect()
    }

    /// แก้ค่าของเซิร์ฟเวอร์แล้วบันทึกลงไฟล์ทันที
    pub async fn update(
        &self,
        guild: serenity::GuildId,
        change: impl FnOnce(&mut GuildSettings),
    ) -> Result<(), Error> {
        let _guard = self.write_lock.lock().await;
        let json = {
            let mut map = self.guilds.write().unwrap_or_else(|e| e.into_inner());
            change(map.entry(guild.get()).or_default());
            serde_json::to_string_pretty(&*map)?
        };
        self.write(json)
            .await
            .map_err(|e| format!("บันทึกการตั้งค่าลงไฟล์ไม่สำเร็จ: {e}").into())
    }

    /// เขียนไฟล์ชั่วคราวก่อนแล้วค่อยเปลี่ยนชื่อ กันไฟล์เสียถ้าบอทดับกลางคัน
    async fn write(&self, json: String) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(dir).await?;
        }
        let tmp = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp, json).await?;
        tokio::fs::rename(&tmp, &self.path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("discord_bot_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("settings.json")
    }

    #[tokio::test]
    async fn saves_and_reloads() {
        let path = temp_path("roundtrip");
        let guild = serenity::GuildId::new(123);
        let settings = Settings::load(&path);
        settings
            .update(guild, |s| s.leave_log = Some(456))
            .await
            .unwrap();

        let reloaded = Settings::load(&path);
        assert_eq!(reloaded.get(guild).leave_log, Some(456));
        assert!(reloaded.with_stats().is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn old_member_log_becomes_join_log() {
        let old: GuildSettings = serde_json::from_str(r#"{"member_log": 7}"#).unwrap();
        assert_eq!(old.join_log, Some(7));
        assert_eq!(old.leave_log, None);
    }

    #[test]
    fn broken_file_is_backed_up_not_fatal() {
        let path = temp_path("broken");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();

        let settings = Settings::load(&path);
        assert!(settings.get(serenity::GuildId::new(1)).join_log.is_none());
        assert!(path.with_extension("json.broken").exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
