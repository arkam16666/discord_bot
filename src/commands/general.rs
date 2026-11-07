use std::time::{Duration, Instant};

use poise::CreateReply;
use poise::serenity_prelude::{self as serenity, Mentionable};

use super::COLOR;
use crate::{Context, Error};

/// แปลง Duration เป็นข้อความ เช่น "2 วัน 3 ชม. 15 นาที"
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let (days, hours, mins, secs) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days} วัน"));
    }
    if hours > 0 {
        parts.push(format!("{hours} ชม."));
    }
    if mins > 0 {
        parts.push(format!("{mins} นาที"));
    }
    if parts.is_empty() || secs > 0 && days == 0 {
        parts.push(format!("{secs} วินาที"));
    }
    parts.join(" ")
}

/// ลำดับหมวดคำสั่งในหน้า help
const CATEGORIES: [&str; 6] = ["เพลง", "สมาชิก", "ทั่วไป", "สนุกๆ", "ดูแลเซิร์ฟเวอร์", "ตั้งค่า"];

/// ดูคำสั่งทั้งหมด หรือวิธีใช้ของคำสั่งที่ระบุ
#[poise::command(slash_command, prefix_command, category = "ทั่วไป")]
pub async fn help(
    ctx: Context<'_>,
    #[description = "ชื่อคำสั่งที่อยากดูวิธีใช้"]
    #[autocomplete = "poise::builtins::autocomplete_command"]
    command: Option<String>,
) -> Result<(), Error> {
    // ระบุคำสั่ง: แสดงวิธีใช้แบบละเอียดของคำสั่งนั้น
    if let Some(command) = command {
        let config = poise::builtins::HelpConfiguration {
            ephemeral: true,
            show_subcommands: true,
            ..Default::default()
        };
        poise::builtins::help(ctx, Some(&command), config).await?;
        return Ok(());
    }

    let mut embed = serenity::CreateEmbed::new()
        .title("คำสั่งทั้งหมด")
        .description(format!(
            "พิมพ์ `/` แล้วเลือกคำสั่งได้เลย หรือพิมพ์ `{}คำสั่ง` ก็ได้\n\
             เริ่มฟังเพลง: `/play ชื่อเพลง` แล้วใช้ปุ่มใต้ข้อความควบคุม",
            ctx.data().prefix
        ))
        .color(COLOR);
    let commands = &ctx.framework().options().commands;
    for category in CATEGORIES {
        let lines: Vec<String> = commands
            .iter()
            .filter(|c| c.category.as_deref() == Some(category) && !c.hide_in_help)
            .map(|c| {
                format!(
                    "`/{}` {}",
                    c.name,
                    c.description.as_deref().unwrap_or_default()
                )
            })
            .collect();
        if !lines.is_empty() {
            embed = embed.field(category, super::truncate(&lines.join("\n"), 1024), false);
        }
    }
    ctx.send(CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}

/// เช็คความเร็วในการตอบสนองของบอท
#[poise::command(slash_command, prefix_command, category = "ทั่วไป")]
pub async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    let start = Instant::now();
    let reply = ctx.say("กำลังวัด...").await?;
    let round_trip = start.elapsed().as_millis();
    let gateway = ctx.ping().await.as_millis();

    let gateway_text = if gateway == 0 {
        "ยังวัดไม่ได้ (รอสักครู่)".to_string()
    } else {
        format!("{gateway} ms")
    };
    reply
        .edit(
            ctx,
            CreateReply::default().content(format!(
                "Pong!\n• API: **{round_trip} ms**\n• Gateway: **{gateway_text}**"
            )),
        )
        .await?;
    Ok(())
}

/// ข้อมูลของบอท
#[poise::command(slash_command, prefix_command, category = "ทั่วไป", aliases("info"))]
pub async fn botinfo(ctx: Context<'_>) -> Result<(), Error> {
    let bot = ctx.cache().current_user().clone();
    let guilds = ctx.cache().guild_count();
    let uptime = format_duration(ctx.data().start_time.elapsed());

    let embed = serenity::CreateEmbed::new()
        .title(bot.name.clone())
        .thumbnail(bot.face())
        .color(COLOR)
        .field("เซิร์ฟเวอร์", guilds.to_string(), true)
        .field(
            "คำสั่ง",
            ctx.framework().options().commands.len().to_string(),
            true,
        )
        .field("Prefix", format!("`{}`", ctx.data().prefix), true)
        .field("ออนไลน์มาแล้ว", uptime, false)
        .footer(serenity::CreateEmbedFooter::new(format!(
            "เวอร์ชัน {} • เขียนด้วย Rust + serenity/poise",
            env!("CARGO_PKG_VERSION")
        )));
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// ดูข้อมูลของผู้ใช้
#[poise::command(
    slash_command,
    prefix_command,
    category = "ทั่วไป",
    aliases("user", "whois")
)]
pub async fn userinfo(
    ctx: Context<'_>,
    #[description = "ผู้ใช้ที่ต้องการดู (ไม่ใส่ = ตัวเอง)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let user = user.as_ref().unwrap_or_else(|| ctx.author());

    let mut embed = serenity::CreateEmbed::new()
        .title(user.tag())
        .thumbnail(user.face())
        .color(COLOR)
        .field("ID", user.id.to_string(), true)
        .field(
            "บอท?",
            if user.bot {
                "ใช่"
            } else {
                "ไม่ใช่"
            },
            true,
        )
        .field(
            "สร้างบัญชีเมื่อ",
            format!("<t:{0}:F>\n(<t:{0}:R>)", user.created_at().unix_timestamp()),
            false,
        );

    // ข้อมูลเพิ่มเติมเมื่อใช้ในเซิร์ฟเวอร์
    if let Some(guild_id) = ctx.guild_id()
        && let Ok(member) = guild_id.member(ctx, user.id).await
    {
        if let Some(joined) = member.joined_at {
            embed = embed.field(
                "เข้าเซิร์ฟเวอร์เมื่อ",
                format!("<t:{0}:F>\n(<t:{0}:R>)", joined.unix_timestamp()),
                false,
            );
        }
        if let Some(nick) = &member.nick {
            embed = embed.field("ชื่อเล่น", nick, true);
        }
        let roles = if member.roles.is_empty() {
            "ไม่มี".to_string()
        } else {
            member
                .roles
                .iter()
                .take(20)
                .map(|r| r.mention().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        };
        embed = embed.field(format!("ยศ ({})", member.roles.len()), roles, false);
    }

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// ดูข้อมูลของเซิร์ฟเวอร์นี้
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "ทั่วไป",
    aliases("server", "guildinfo")
)]
pub async fn serverinfo(ctx: Context<'_>) -> Result<(), Error> {
    // ดึงข้อมูลจาก cache ภายในบล็อก เพราะ reference ของ cache ห้ามถือข้าม .await
    let embed = {
        let guild = ctx.guild().ok_or("ไม่พบข้อมูลเซิร์ฟเวอร์ใน cache")?;
        let text_channels = guild
            .channels
            .values()
            .filter(|c| c.kind == serenity::ChannelType::Text)
            .count();
        let voice_channels = guild
            .channels
            .values()
            .filter(|c| c.kind == serenity::ChannelType::Voice)
            .count();

        let mut embed = serenity::CreateEmbed::new()
            .title(guild.name.clone())
            .color(COLOR)
            .field("เจ้าของ", guild.owner_id.mention().to_string(), true)
            .field("สมาชิก", guild.member_count.to_string(), true)
            .field("ยศ", guild.roles.len().to_string(), true)
            .field(
                "ช่อง",
                format!("{text_channels} ข้อความ • {voice_channels} เสียง"),
                true,
            )
            .field(
                "บูสต์",
                format!(
                    "{} ครั้ง (เลเวล {})",
                    guild.premium_subscription_count.unwrap_or(0),
                    u8::from(guild.premium_tier)
                ),
                true,
            )
            .field(
                "สร้างเมื่อ",
                format!(
                    "<t:{0}:F>\n(<t:{0}:R>)",
                    guild.id.created_at().unix_timestamp()
                ),
                false,
            )
            .footer(serenity::CreateEmbedFooter::new(format!(
                "ID: {}",
                guild.id
            )));
        if let Some(icon) = guild.icon_url() {
            embed = embed.thumbnail(icon);
        }
        embed
    };

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// ดูรูปโปรไฟล์ของผู้ใช้แบบขนาดใหญ่
#[poise::command(slash_command, prefix_command, category = "ทั่วไป", aliases("av", "pfp"))]
pub async fn avatar(
    ctx: Context<'_>,
    #[description = "ผู้ใช้ที่ต้องการดู (ไม่ใส่ = ตัวเอง)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let user = user.as_ref().unwrap_or_else(|| ctx.author());
    let url = user.face();
    let embed = serenity::CreateEmbed::new()
        .title(format!("รูปโปรไฟล์ของ {}", user.name))
        .url(&url)
        .image(&url)
        .color(COLOR);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}
