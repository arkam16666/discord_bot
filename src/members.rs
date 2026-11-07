//! นับสมาชิก/คนออนไลน์ และแจ้งเตือนคนเข้า-ออก

use poise::serenity_prelude::{self as serenity, Mentionable};

use crate::commands::{escape, truncate};
use crate::settings::{LogKind, Settings};
use crate::{Data, Error};

/// บัญชีที่สร้างไม่ถึงกี่วันถือว่าเป็นบัญชีใหม่ (ไว้เตือนแอดมิน)
const NEW_ACCOUNT_DAYS: i64 = 7;

/// ฟีเจอร์ที่ใช้ได้ ขึ้นกับ intent ที่เปิดใน Developer Portal
#[derive(Debug, Clone, Copy)]
pub struct Features {
    /// SERVER MEMBERS INTENT: แจ้งเตือนคนเข้า/ออก, นับคน/บอท
    pub members: bool,
    /// PRESENCE INTENT: นับคนออนไลน์
    pub presences: bool,
    /// MESSAGE CONTENT INTENT: คำสั่งแบบ prefix
    pub message_content: bool,
}

impl Features {
    /// intent ที่ต้องขอจาก Discord สำหรับฟีเจอร์ชุดนี้
    pub fn intents(self) -> serenity::GatewayIntents {
        use serenity::GatewayIntents as I;
        let mut intents = I::non_privileged();
        if self.message_content {
            intents |= I::MESSAGE_CONTENT;
        }
        if self.members {
            intents |= I::GUILD_MEMBERS;
        }
        if self.presences {
            intents |= I::GUILD_PRESENCES;
        }
        intents
    }
}

/// ตัวเลขสถิติของเซิร์ฟเวอร์
pub struct MemberCounts {
    pub total: u64,
    /// จำนวนคน/บอท (รู้เฉพาะเมื่อโหลดรายชื่อสมาชิกครบ)
    pub humans: Option<u64>,
    pub bots: Option<u64>,
    /// จำนวนคนออนไลน์ (ไม่นับบอท) แยกตามสถานะ
    pub online: Option<OnlineCounts>,
}

#[derive(Default)]
pub struct OnlineCounts {
    pub online: u64,
    pub idle: u64,
    pub dnd: u64,
}

impl OnlineCounts {
    pub fn total(&self) -> u64 {
        self.online + self.idle + self.dnd
    }
}

/// นับจาก cache ของเซิร์ฟเวอร์
pub fn count(guild: &serenity::Guild, features: Features) -> MemberCounts {
    let total = guild.member_count;
    let members_complete = features.members && guild.members.len() as u64 >= total;

    let (humans, bots) = if members_complete {
        let bots = guild.members.values().filter(|m| m.user.bot).count() as u64;
        (Some(total.saturating_sub(bots)), Some(bots))
    } else {
        (None, None)
    };

    let online = features.presences.then(|| {
        let mut counts = OnlineCounts::default();
        for (user_id, presence) in &guild.presences {
            let is_bot = presence
                .user
                .bot
                .or_else(|| guild.members.get(user_id).map(|m| m.user.bot))
                .unwrap_or(false);
            if is_bot {
                continue;
            }
            match presence.status {
                serenity::OnlineStatus::Online => counts.online += 1,
                serenity::OnlineStatus::Idle => counts.idle += 1,
                serenity::OnlineStatus::DoNotDisturb => counts.dnd += 1,
                _ => {}
            }
        }
        counts
    });

    MemberCounts {
        total,
        humans,
        bots,
        online,
    }
}

/// ห้องแจ้งเตือนของเซิร์ฟเวอร์นี้ (ต้องยังมีอยู่ในเซิร์ฟเวอร์)
fn log_channel(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    kind: LogKind,
) -> Option<serenity::ChannelId> {
    let channel = data.settings.get(guild_id).log(kind)?;
    let guild = ctx.cache.guild(guild_id)?;
    guild.channels.contains_key(&channel).then_some(channel)
}

fn member_total(ctx: &serenity::Context, guild_id: serenity::GuildId) -> Option<u64> {
    ctx.cache.guild(guild_id).map(|g| g.member_count)
}

/// แจ้งเตือนเมื่อมีคนเข้าเซิร์ฟเวอร์
pub async fn on_join(
    ctx: &serenity::Context,
    data: &Data,
    member: &serenity::Member,
) -> Result<(), Error> {
    let Some(channel) = log_channel(ctx, data, member.guild_id, LogKind::Join) else {
        return Ok(());
    };

    let created = member.user.created_at().unix_timestamp();
    let age_days = (serenity::Timestamp::now().unix_timestamp() - created) / 86400;
    let mut account = format!("<t:{created}:R>");
    if age_days < NEW_ACCOUNT_DAYS {
        account.push_str(" (บัญชีใหม่)");
    }

    let mut embed = serenity::CreateEmbed::new()
        .title("สมาชิกใหม่เข้าร่วม")
        .description(format!("ยินดีต้อนรับ {} สู่เซิร์ฟเวอร์!", member.mention()))
        .thumbnail(member.face())
        .color(0x57F287)
        .field("ชื่อ", escape(&member.user.tag()), true)
        .field("สร้างบัญชีเมื่อ", account, true);
    if let Some(total) = member_total(ctx, member.guild_id) {
        embed = embed.footer(serenity::CreateEmbedFooter::new(format!(
            "ตอนนี้มีสมาชิก {total} คน"
        )));
    }

    channel
        .send_message(
            ctx,
            serenity::CreateMessage::new()
                .content(member.mention().to_string())
                .embed(embed)
                .allowed_mentions(serenity::CreateAllowedMentions::new().users([member.user.id])),
        )
        .await?;
    Ok(())
}

/// แจ้งเตือนเมื่อมีคนออกจากเซิร์ฟเวอร์ (ออกเอง ถูกเตะ หรือถูกแบน)
pub async fn on_leave(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    user: &serenity::User,
    member: Option<&serenity::Member>,
) -> Result<(), Error> {
    let Some(channel) = log_channel(ctx, data, guild_id, LogKind::Leave) else {
        return Ok(());
    };

    let mut embed = serenity::CreateEmbed::new()
        .title("สมาชิกออกจากเซิร์ฟเวอร์")
        .description(format!(
            "**{}** ออกจากเซิร์ฟเวอร์แล้ว",
            escape(&truncate(&user.tag(), 100))
        ))
        .thumbnail(user.face())
        .color(0xED4245)
        .field("ID", user.id.to_string(), true);
    if let Some(joined) = member.and_then(|m| m.joined_at) {
        embed = embed.field(
            "เข้าร่วมเมื่อ",
            format!("<t:{}:R>", joined.unix_timestamp()),
            true,
        );
    }
    if let Some(total) = member_total(ctx, guild_id) {
        embed = embed.footer(serenity::CreateEmbedFooter::new(format!(
            "เหลือสมาชิก {total} คน"
        )));
    }

    channel
        .send_message(ctx, serenity::CreateMessage::new().embed(embed))
        .await?;
    Ok(())
}

/// ตั้งห้องแจ้งเตือนคนเข้าหรือคนออก (None = ปิด) ใช้ร่วมกันทั้งคำสั่งและหน้าเว็บ
pub async fn set_log(
    ctx: &serenity::Context,
    settings: &Settings,
    guild_id: serenity::GuildId,
    features: Features,
    kind: LogKind,
    channel: Option<serenity::ChannelId>,
) -> Result<String, Error> {
    let label = kind.label();
    let Some(channel) = channel else {
        settings.update(guild_id, |s| s.set_log(kind, None)).await?;
        return Ok(format!("ปิดการแจ้งเตือน{label}แล้ว"));
    };
    let is_text = ctx.cache.guild(guild_id).is_some_and(|g| {
        g.channels.get(&channel).is_some_and(|c| {
            matches!(
                c.kind,
                serenity::ChannelType::Text | serenity::ChannelType::News
            )
        })
    });
    if !is_text {
        return Err("ต้องเลือกห้องแชทในเซิร์ฟเวอร์นี้".into());
    }
    if !features.members {
        return Err(
            "ต้องเปิด SERVER MEMBERS INTENT ใน Developer Portal แล้วรีสตาร์ทบอทก่อน \
                    ไม่งั้นบอทจะไม่รู้ว่ามีคนเข้า/ออก"
                .into(),
        );
    }

    // ลองส่งข้อความจริงก่อนบันทึก ถ้าบอทไม่มีสิทธิ์จะได้รู้ทันที
    channel
        .say(ctx, format!("ห้องนี้ถูกตั้งเป็นห้องแจ้งเตือน{label}แล้ว"))
        .await
        .map_err(|e| {
            format!(
                "บอทส่งข้อความในห้อง {} ไม่ได้ (ตรวจสิทธิ์ View Channel, Send Messages, Embed Links): {e}",
                channel.mention()
            )
        })?;
    settings
        .update(guild_id, |s| s.set_log(kind, Some(channel)))
        .await?;
    Ok(format!("ตั้งห้องแจ้งเตือน{label}เป็น {} แล้ว", channel.mention()))
}

/// ขอรายชื่อสมาชิกทั้งหมดจาก Discord (เซิร์ฟเวอร์ใหญ่จะได้รายชื่อไม่ครบตอนเริ่ม)
pub fn request_members(ctx: &serenity::Context, guild: &serenity::Guild, features: Features) {
    if features.members && (guild.members.len() as u64) < guild.member_count {
        ctx.shard.chunk_guild(
            guild.id,
            None,
            features.presences,
            serenity::ChunkGuildFilter::None,
            None,
        );
    }
}
