//! ห้องแสดงจำนวนสมาชิก/คนออนไลน์: ใช้ห้องที่มีอยู่แล้ว หรือให้บอทสร้างให้
//! ใช้ร่วมกันทั้งคำสั่ง /setup stats และหน้าเว็บจัดการ

use std::sync::Arc;
use std::time::Duration;

use poise::serenity_prelude::{self as serenity, Mentionable};

use crate::Error;
use crate::commands::truncate;
use crate::members::{self, Features, MemberCounts};
use crate::settings::{GuildSettings, Settings};

/// อัปเดตชื่อห้องสถิติทุกๆ 5 นาที
/// (Discord ให้เปลี่ยนชื่อห้องได้แค่ 2 ครั้งต่อ 10 นาที และบอทจะเปลี่ยนเฉพาะตอนตัวเลขเปลี่ยน)
const STATS_INTERVAL: Duration = Duration::from_secs(300);
/// ความยาวสูงสุดของรูปแบบชื่อห้อง (Discord จำกัดชื่อห้อง 100 ตัวอักษร)
pub const MAX_TEMPLATE: usize = 90;

/// ชนิดห้องที่ใช้แสดงสถิติได้ (ห้องแชทใช้ไม่ได้ เพราะ Discord ห้ามมีช่องว่างและ : ในชื่อ)
pub fn usable(kind: serenity::ChannelType) -> bool {
    use serenity::ChannelType as T;
    matches!(kind, T::Voice | T::Stage | T::Category)
}

fn fill(template: &str, value: &str) -> String {
    truncate(&template.replace("{count}", value), 100)
}

pub fn total_name(config: &GuildSettings, counts: &MemberCounts) -> String {
    fill(config.total_template(), &counts.total.to_string())
}

pub fn online_name(config: &GuildSettings, counts: &MemberCounts) -> String {
    let value = counts
        .online
        .as_ref()
        .map_or_else(|| "?".to_string(), |o| o.total().to_string());
    fill(config.online_template(), &value)
}

/// อัปเดตชื่อห้องสถิติของเซิร์ฟเวอร์ (เปลี่ยนเฉพาะห้องที่ชื่อไม่ตรง)
pub async fn update(
    ctx: &serenity::Context,
    settings: &Settings,
    guild_id: serenity::GuildId,
    features: Features,
) -> Result<(), Error> {
    let config = settings.get(guild_id);

    // อ่าน cache ให้เสร็จในบล็อก (ห้ามถือ cache ข้าม .await)
    let (renames, missing_total, missing_online) = {
        let Some(guild) = ctx.cache.guild(guild_id) else {
            return Ok(());
        };
        let counts = members::count(&guild, features);
        let mut renames = Vec::new();
        let mut check = |id: Option<u64>, name: String| -> bool {
            let Some(id) = id.map(serenity::ChannelId::new) else {
                return false;
            };
            match guild.channels.get(&id) {
                Some(channel) if channel.name != name => renames.push((id, name)),
                Some(_) => {}
                None => return true,
            }
            false
        };
        let missing_total = check(config.stats_total, total_name(&config, &counts));
        let missing_online = check(config.stats_online, online_name(&config, &counts));
        (renames, missing_total, missing_online)
    };

    // มีคนลบห้องสถิติไป: เลิกอัปเดตห้องนั้น
    if missing_total || missing_online {
        eprintln!("[สถิติ] ห้องสถิติบางห้องในเซิร์ฟเวอร์ {guild_id} ถูกลบไป จะเลิกอัปเดตห้องนั้น");
        settings
            .update(guild_id, |s| {
                if missing_total {
                    s.stats_total = None;
                }
                if missing_online {
                    s.stats_online = None;
                }
                if !s.has_stats() {
                    s.stats_category = None;
                }
            })
            .await?;
    }

    for (channel, name) in renames {
        channel
            .edit(ctx, serenity::EditChannel::new().name(name))
            .await
            .map_err(|e| format!("เปลี่ยนชื่อห้อง {} ไม่สำเร็จ: {e}", channel.mention()))?;
    }
    Ok(())
}

/// อัปเดตชื่อห้องทันทีในเบื้องหลัง (ถ้าติด rate limit ของ Discord จะรอเอง ไม่ทำให้คำสั่งค้าง)
pub fn update_soon(
    ctx: &serenity::Context,
    settings: Arc<Settings>,
    guild_id: serenity::GuildId,
    features: Features,
) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = update(&ctx, &settings, guild_id, features).await {
            eprintln!("[สถิติ] {e}");
        }
    });
}

/// งานเบื้องหลัง: อัปเดตห้องสถิติของทุกเซิร์ฟเวอร์เป็นระยะ
pub async fn run_loop(ctx: serenity::Context, settings: Arc<Settings>, features: Features) {
    let mut interval = tokio::time::interval(STATS_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // รอบแรกรอให้ cache โหลดเซิร์ฟเวอร์ครบก่อน
    tokio::time::sleep(Duration::from_secs(15)).await;
    loop {
        interval.tick().await;
        for (guild_id, _) in settings.with_stats() {
            if let Err(e) = update(&ctx, &settings, guild_id, features).await {
                eprintln!("[สถิติ] อัปเดตเซิร์ฟเวอร์ {guild_id} ไม่สำเร็จ: {e}");
            }
        }
    }
}

/// ตรวจว่าห้องนี้ใช้แสดงสถิติได้ และบอทมีสิทธิ์เปลี่ยนชื่อ
fn check_channel(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    id: serenity::ChannelId,
) -> Result<(), Error> {
    let guild = ctx
        .cache
        .guild(guild_id)
        .ok_or("ไม่พบข้อมูลเซิร์ฟเวอร์ใน cache")?;
    let channel = guild.channels.get(&id).ok_or("ไม่พบห้องที่เลือกในเซิร์ฟเวอร์นี้")?;
    if !usable(channel.kind) {
        return Err(format!(
            "ใช้ห้อง {} ไม่ได้ ต้องเป็นห้องเสียงหรือหมวดหมู่ (ห้องแชทตั้งชื่อมีช่องว่างไม่ได้)",
            id.mention()
        )
        .into());
    }
    let bot_id = ctx.cache.current_user().id;
    if let Some(bot) = guild.members.get(&bot_id)
        && !guild.user_permissions_in(channel, bot).manage_channels()
    {
        return Err(format!(
            "บอทไม่มีสิทธิ์ Manage Channels ในห้อง {} จึงเปลี่ยนชื่อห้องไม่ได้",
            id.mention()
        )
        .into());
    }
    Ok(())
}

/// ใช้ห้องที่มีอยู่แล้วแสดงสถิติ (ไม่ใช้ห้องไหน ใส่ None)
pub async fn use_existing(
    ctx: &serenity::Context,
    settings: &Arc<Settings>,
    guild_id: serenity::GuildId,
    features: Features,
    total: Option<serenity::ChannelId>,
    online: Option<serenity::ChannelId>,
) -> Result<String, Error> {
    if total.is_none() && online.is_none() {
        return disable(ctx, settings, guild_id).await;
    }
    if total.is_some() && total == online {
        return Err("ห้องจำนวนสมาชิกกับห้องคนออนไลน์ต้องเป็นคนละห้องกัน".into());
    }
    for id in [total, online].into_iter().flatten() {
        check_channel(ctx, guild_id, id)?;
    }

    let old = settings.get(guild_id);
    let old_ids = [old.stats_total, old.stats_online];
    let new_ids = [total.map(|c| c.get()), online.map(|c| c.get())];
    if old_ids != new_ids {
        // เคยให้บอทสร้างห้องไว้: ลบห้องเก่าที่ไม่ได้ใช้แล้ว
        if old.stats_category.is_some() {
            let keep: Vec<u64> = new_ids.into_iter().flatten().collect();
            let unused = old_ids
                .into_iter()
                .flatten()
                .filter(|id| !keep.contains(id));
            for id in unused {
                let _ = serenity::ChannelId::new(id).delete(ctx).await;
            }
        }
        settings
            .update(guild_id, |s| {
                s.stats_category = None;
                s.stats_total = new_ids[0];
                s.stats_online = new_ids[1];
            })
            .await?;
    }
    update_soon(ctx, settings.clone(), guild_id, features);

    let list: Vec<String> = [total, online]
        .into_iter()
        .flatten()
        .map(|c| c.mention().to_string())
        .collect();
    let mut text = format!(
        "ใช้ห้อง {} แสดงสถิติแล้ว (เปลี่ยนชื่อทุก 5 นาทีเมื่อตัวเลขเปลี่ยน)",
        list.join(" และ ")
    );
    if online.is_some() && !features.presences {
        text.push_str("\nต้องเปิด PRESENCE INTENT ใน Developer Portal ถึงจะนับคนออนไลน์ได้");
    }
    Ok(text)
}

/// ให้บอทสร้างหมวดหมู่และห้องสถิติใหม่ (ทุกคนเห็นแต่เข้าห้องไม่ได้)
pub async fn create(
    ctx: &serenity::Context,
    settings: &Arc<Settings>,
    guild_id: serenity::GuildId,
    features: Features,
) -> Result<String, Error> {
    let config = settings.get(guild_id);
    let (total_name, online_name) = {
        let guild = ctx
            .cache
            .guild(guild_id)
            .ok_or("ไม่พบข้อมูลเซิร์ฟเวอร์ใน cache")?;
        let still_there = [config.stats_total, config.stats_online]
            .into_iter()
            .flatten()
            .any(|id| guild.channels.contains_key(&serenity::ChannelId::new(id)));
        if still_there {
            return Err("ห้องสถิติเปิดอยู่แล้ว ถ้าจะเปลี่ยน ให้ปิดก่อน".into());
        }
        let counts = members::count(&guild, features);
        (total_name(&config, &counts), online_name(&config, &counts))
    };

    let overwrites = vec![
        serenity::PermissionOverwrite {
            allow: serenity::Permissions::VIEW_CHANNEL,
            deny: serenity::Permissions::CONNECT,
            kind: serenity::PermissionOverwriteType::Role(guild_id.everyone_role()),
        },
        serenity::PermissionOverwrite {
            allow: serenity::Permissions::VIEW_CHANNEL
                | serenity::Permissions::CONNECT
                | serenity::Permissions::MANAGE_CHANNELS,
            deny: serenity::Permissions::empty(),
            kind: serenity::PermissionOverwriteType::Member(ctx.cache.current_user().id),
        },
    ];

    let category = guild_id
        .create_channel(
            ctx,
            serenity::CreateChannel::new("สถิติเซิร์ฟเวอร์")
                .kind(serenity::ChannelType::Category)
                .position(0),
        )
        .await
        .map_err(|e| format!("สร้างหมวดหมู่ไม่สำเร็จ (บอทต้องมีสิทธิ์ Manage Channels): {e}"))?;

    let mut created = Vec::new();
    for name in [total_name, online_name] {
        let result = guild_id
            .create_channel(
                ctx,
                serenity::CreateChannel::new(name)
                    .kind(serenity::ChannelType::Voice)
                    .category(category.id)
                    .permissions(overwrites.clone()),
            )
            .await;
        match result {
            Ok(channel) => created.push(channel.id),
            Err(e) => {
                // สร้างไม่ครบ: ลบที่สร้างไปแล้วทิ้ง ไม่ให้เหลือห้องค้าง
                for id in created {
                    let _ = id.delete(ctx).await;
                }
                let _ = category.id.delete(ctx).await;
                return Err(format!("สร้างห้องสถิติไม่สำเร็จ: {e}").into());
            }
        }
    }

    let (total_id, online_id) = (created[0], created[1]);
    settings
        .update(guild_id, |s| {
            s.stats_category = Some(category.id.get());
            s.stats_total = Some(total_id.get());
            s.stats_online = Some(online_id.get());
        })
        .await?;

    let mut text = format!(
        "สร้างห้องสถิติแล้ว: {} และ {} (อัปเดตทุก 5 นาที)",
        total_id.mention(),
        online_id.mention()
    );
    if !features.presences {
        text.push_str("\nต้องเปิด PRESENCE INTENT ใน Developer Portal ถึงจะนับคนออนไลน์ได้");
    }
    Ok(text)
}

/// เลิกแสดงสถิติ: ห้องที่บอทสร้างจะถูกลบ ห้องที่มีอยู่เดิมจะเก็บไว้ (แค่เลิกเปลี่ยนชื่อ)
pub async fn disable(
    ctx: &serenity::Context,
    settings: &Settings,
    guild_id: serenity::GuildId,
) -> Result<String, Error> {
    let config = settings.get(guild_id);
    if !config.has_stats() && config.stats_category.is_none() {
        return Err("ห้องสถิติปิดอยู่แล้ว".into());
    }

    let mut failed = Vec::new();
    if config.stats_category.is_some() {
        let ids = [
            config.stats_total,
            config.stats_online,
            config.stats_category,
        ];
        for channel in ids.into_iter().flatten().map(serenity::ChannelId::new) {
            let exists = ctx
                .cache
                .guild(guild_id)
                .is_some_and(|g| g.channels.contains_key(&channel));
            if exists && let Err(e) = channel.delete(ctx).await {
                failed.push(format!("{}: {e}", channel.mention()));
            }
        }
    }

    settings
        .update(guild_id, |s| {
            s.stats_category = None;
            s.stats_total = None;
            s.stats_online = None;
        })
        .await?;

    Ok(if config.stats_category.is_none() {
        "เลิกอัปเดตห้องสถิติแล้ว (ห้องยังอยู่ เปลี่ยนชื่อกลับเองได้)".to_string()
    } else if failed.is_empty() {
        "ปิดและลบห้องสถิติที่บอทสร้างไว้แล้ว".to_string()
    } else {
        format!(
            "ปิดการอัปเดตแล้ว แต่ลบบางห้องไม่สำเร็จ (ลบเองได้เลย):\n{}",
            failed.join("\n")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(online: Option<u64>) -> MemberCounts {
        MemberCounts {
            total: 120,
            humans: None,
            bots: None,
            online: online.map(|n| members::OnlineCounts {
                online: n,
                ..Default::default()
            }),
        }
    }

    #[test]
    fn fills_name_templates() {
        let mut config = GuildSettings::default();
        assert_eq!(total_name(&config, &counts(None)), "สมาชิกทั้งหมด: 120");
        assert_eq!(online_name(&config, &counts(None)), "ออนไลน์: ?");

        config.total_name = Some("Members {count} คน".into());
        config.online_name = Some("{count} online".into());
        assert_eq!(total_name(&config, &counts(None)), "Members 120 คน");
        assert_eq!(online_name(&config, &counts(Some(7))), "7 online");
    }
}
