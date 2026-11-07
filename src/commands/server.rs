use poise::CreateReply;
use poise::serenity_prelude::{self as serenity, Mentionable};

use super::COLOR;
use crate::members::{self, MemberCounts};
use crate::settings::LogKind;
use crate::stats;
use crate::{Context, Error};

/// ดูจำนวนสมาชิกทั้งหมดและคนที่ออนไลน์
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "สมาชิก",
    aliases("member", "online")
)]
pub async fn members(ctx: Context<'_>) -> Result<(), Error> {
    let features = ctx.data().features;
    let (name, icon, counts) = {
        let guild = ctx.guild().ok_or("ไม่พบข้อมูลเซิร์ฟเวอร์ใน cache ลองใหม่อีกครั้ง")?;
        (
            guild.name.clone(),
            guild.icon_url(),
            members::count(&guild, features),
        )
    };
    let MemberCounts {
        total,
        humans,
        bots,
        online,
    } = counts;

    let mut embed = serenity::CreateEmbed::new()
        .title(format!("สมาชิกของ {name}"))
        .color(COLOR)
        .field("ทั้งหมด", total.to_string(), true);

    embed = match (humans, bots) {
        (Some(h), Some(b)) => {
            embed
                .field("คน", h.to_string(), true)
                .field("บอท", b.to_string(), true)
        }
        _ if !features.members => embed.field("คน / บอท", "ต้องเปิด SERVER MEMBERS INTENT", true),
        _ => embed.field("คน / บอท", "กำลังโหลดรายชื่อ ลองใหม่สักครู่", true),
    };

    embed = match online {
        Some(o) => {
            let offline = humans
                .map(|h| h.saturating_sub(o.total()).to_string())
                .unwrap_or_else(|| "ไม่ทราบ".to_string());
            embed
                .field("ออนไลน์", o.total().to_string(), true)
                .field("ออฟไลน์", offline, true)
                .field(
                    "แยกตามสถานะ",
                    format!("ออนไลน์ {} • ไม่อยู่ {} • ห้ามรบกวน {}", o.online, o.idle, o.dnd),
                    false,
                )
        }
        None => embed.field("ออนไลน์", "ต้องเปิด PRESENCE INTENT", false),
    };
    if let Some(icon) = icon {
        embed = embed.thumbnail(icon);
    }
    embed = embed.footer(serenity::CreateEmbedFooter::new("ไม่นับบอทในจำนวนคนออนไลน์"));

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// ตั้งค่าบอทสำหรับเซิร์ฟเวอร์นี้
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "ตั้งค่า",
    subcommands("setup_status", "setup_welcome", "setup_goodbye", "setup_stats"),
    subcommand_required,
    required_permissions = "MANAGE_GUILD"
)]
pub async fn setup(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// ดูการตั้งค่าทั้งหมด และสิ่งที่ยังต้องทำ
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    rename = "status",
    required_permissions = "MANAGE_GUILD"
)]
pub async fn setup_status(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;
    let data = ctx.data();
    let config = data.settings.get(guild_id);
    let on_off = |on: bool| if on { "เปิด" } else { "ปิด" };

    let log = |kind: LogKind, command: &str| match config.log(kind) {
        Some(channel) => channel.mention().to_string(),
        None => format!("ยังไม่ได้ตั้ง → `/setup {command}`"),
    };
    let logs = format!(
        "คนเข้า: {}
คนออก: {}",
        log(LogKind::Join, "welcome"),
        log(LogKind::Leave, "goodbye")
    );
    let stats = if config.has_stats() {
        "เปิดอยู่".to_string()
    } else {
        "ปิดอยู่ → `/setup stats เปิด`".to_string()
    };
    let f = data.features;
    let intents = format!(
        "SERVER MEMBERS: {}\nPRESENCE: {}\nMESSAGE CONTENT: {}",
        on_off(f.members),
        on_off(f.presences),
        on_off(f.message_content)
    );
    let music = format!(
        "yt-dlp: {}\nJavaScript runtime: {}",
        if data.music.ytdlp_ready() {
            "พร้อม"
        } else {
            "ไม่พบ (winget install yt-dlp.yt-dlp)"
        },
        data.music
            .js_runtime
            .unwrap_or("ไม่พบ (winget install DenoLand.Deno)")
    );

    let mut embed = serenity::CreateEmbed::new()
        .title("การตั้งค่า")
        .color(COLOR)
        .field("แจ้งเตือนสมาชิก", logs, false)
        .field("ห้องแสดงจำนวนสมาชิก", stats, false)
        .field("Intents", intents, true)
        .field("ระบบเพลง", music, true);
    if !(f.members && f.presences && f.message_content) {
        embed = embed.footer(serenity::CreateEmbedFooter::new(
            "Intent ที่ปิดอยู่ เปิดได้ที่ Discord Developer Portal > Bot > Privileged Gateway Intents แล้วรีสตาร์ทบอท",
        ));
    }
    ctx.send(CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}

/// ตั้งห้องแจ้งเตือนคนเข้าเซิร์ฟเวอร์ (ไม่เลือกห้อง = ปิด)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    rename = "welcome",
    required_permissions = "MANAGE_GUILD"
)]
pub async fn setup_welcome(
    ctx: Context<'_>,
    #[description = "ห้องที่จะส่งแจ้งเตือนคนเข้า (ไม่เลือก = ปิด)"]
    #[channel_types("Text", "News")]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    set_log(ctx, LogKind::Join, channel).await
}

/// ตั้งห้องแจ้งเตือนคนออกจากเซิร์ฟเวอร์ (ไม่เลือกห้อง = ปิด)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    rename = "goodbye",
    required_permissions = "MANAGE_GUILD"
)]
pub async fn setup_goodbye(
    ctx: Context<'_>,
    #[description = "ห้องที่จะส่งแจ้งเตือนคนออก (ไม่เลือก = ปิด)"]
    #[channel_types("Text", "News")]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    set_log(ctx, LogKind::Leave, channel).await
}

async fn set_log(
    ctx: Context<'_>,
    kind: LogKind,
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;
    let data = ctx.data();
    let text = members::set_log(
        ctx.serenity_context(),
        &data.settings,
        guild_id,
        data.features,
        kind,
        channel.map(|c| c.id),
    )
    .await?;
    ctx.say(text).await?;
    Ok(())
}

#[derive(poise::ChoiceParameter, PartialEq)]
pub enum OnOff {
    #[name = "เปิด"]
    #[name = "on"]
    On,
    #[name = "ปิด"]
    #[name = "off"]
    Off,
}

/// ห้องแสดงจำนวนสมาชิก/คนออนไลน์: เลือกห้องที่มีอยู่แล้ว หรือไม่เลือกเพื่อให้บอทสร้างให้
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    rename = "stats",
    required_permissions = "MANAGE_GUILD"
)]
pub async fn setup_stats(
    ctx: Context<'_>,
    #[description = "เปิดหรือปิด"] mode: OnOff,
    #[description = "ห้องที่มีอยู่แล้ว ใช้แสดงจำนวนสมาชิก (ไม่เลือก = สร้างห้องใหม่)"]
    #[channel_types("Voice", "Stage", "Category")]
    total: Option<serenity::GuildChannel>,
    #[description = "ห้องที่มีอยู่แล้ว ใช้แสดงจำนวนคนออนไลน์"]
    #[channel_types("Voice", "Stage", "Category")]
    online: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    ctx.defer().await?;
    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;
    let data = ctx.data();
    let serenity_ctx = ctx.serenity_context();
    let (total, online) = (total.map(|c| c.id), online.map(|c| c.id));

    let text = match mode {
        OnOff::Off => stats::disable(serenity_ctx, &data.settings, guild_id).await?,
        OnOff::On if total.is_none() && online.is_none() => {
            stats::create(serenity_ctx, &data.settings, guild_id, data.features).await?
        }
        OnOff::On => {
            stats::use_existing(
                serenity_ctx,
                &data.settings,
                guild_id,
                data.features,
                total,
                online,
            )
            .await?
        }
    };
    ctx.say(text).await?;
    Ok(())
}
