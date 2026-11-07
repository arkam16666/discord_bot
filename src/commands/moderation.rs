use std::time::Duration;

use poise::CreateReply;
use poise::serenity_prelude::{self as serenity, Mentionable};

use crate::{Context, Error};

/// Discord ลบข้อความแบบกลุ่มได้เฉพาะข้อความที่อายุไม่เกิน 14 วัน
const BULK_DELETE_MAX_AGE_SECS: i64 = 14 * 24 * 60 * 60;
/// timeout ได้นานสุด 28 วัน
const MAX_TIMEOUT_MINUTES: u32 = 28 * 24 * 60;

/// ตรวจว่าผู้ใช้คำสั่งมีสิทธิ์จัดการเป้าหมายได้จริง (ยศต้องสูงกว่า)
async fn check_hierarchy(ctx: Context<'_>, target: &serenity::User) -> Result<(), Error> {
    if target.id == ctx.author().id {
        return Err("ใช้คำสั่งนี้กับตัวเองไม่ได้".into());
    }
    if target.id == ctx.framework().bot_id {
        return Err("ใช้คำสั่งนี้กับบอทเองไม่ได้".into());
    }

    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;
    // ถ้าเป้าหมายไม่ได้อยู่ในเซิร์ฟเวอร์ (เช่น แบนด้วย ID) ก็ไม่ต้องเช็คยศ
    let Ok(target_member) = guild_id.member(ctx, target.id).await else {
        return Ok(());
    };
    let author_member = ctx.author_member().await.ok_or("ไม่พบข้อมูลสมาชิกของคุณ")?;

    let (owner_id, author_pos, target_pos) = {
        let guild = ctx.guild().ok_or("ไม่พบข้อมูลเซิร์ฟเวอร์ใน cache")?;
        let top_role = |m: &serenity::Member| {
            m.roles
                .iter()
                .filter_map(|id| guild.roles.get(id))
                .map(|r| r.position)
                .max()
                .unwrap_or(0)
        };
        (
            guild.owner_id,
            top_role(&author_member),
            top_role(&target_member),
        )
    };

    if target.id == owner_id {
        return Err("ใช้คำสั่งนี้กับเจ้าของเซิร์ฟเวอร์ไม่ได้".into());
    }
    if ctx.author().id != owner_id && author_pos <= target_pos {
        return Err("ยศของคุณต้องสูงกว่าเป้าหมาย".into());
    }
    Ok(())
}

/// เหตุผลใน audit log ของ Discord ยาวได้ไม่เกิน 512 ตัวอักษร
fn clean_reason(reason: Option<String>) -> Result<String, Error> {
    let reason = reason
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| "ไม่ระบุ".to_string());
    if reason.chars().count() > 400 {
        return Err("เหตุผลยาวได้ไม่เกิน 400 ตัวอักษร".into());
    }
    Ok(reason)
}

/// ส่ง DM แจ้งเป้าหมาย (ถ้าเขาปิด DM ไว้ก็ข้ามไป)
async fn notify(ctx: Context<'_>, user: &serenity::User, action: &str, reason: &str) {
    let guild_name = ctx
        .guild()
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "เซิร์ฟเวอร์".to_string());
    let msg = serenity::CreateMessage::new()
        .content(format!("คุณถูก{action}จาก **{guild_name}**\nเหตุผล: {reason}"));
    let _ = user.direct_message(ctx, msg).await;
}

/// ลบข้อความล่าสุดในช่องนี้ (1-100 ข้อความ)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "ดูแลเซิร์ฟเวอร์",
    aliases("purge"),
    required_permissions = "MANAGE_MESSAGES",
    required_bot_permissions = "MANAGE_MESSAGES | READ_MESSAGE_HISTORY"
)]
pub async fn clear(
    ctx: Context<'_>,
    #[description = "จำนวนข้อความที่จะลบ"]
    #[min = 1]
    #[max = 100]
    amount: u8,
) -> Result<(), Error> {
    match ctx {
        // ลบข้อความคำสั่งของผู้ใช้ก่อน จะได้ไม่นับรวม
        poise::Context::Prefix(prefix_ctx) => {
            let _ = prefix_ctx.msg.delete(ctx).await;
        }
        // slash command ต้องตอบภายใน 3 วินาที จึง defer ไว้ก่อน
        poise::Context::Application(_) => ctx.defer_ephemeral().await?,
    }

    let cutoff = serenity::Timestamp::now().unix_timestamp() - BULK_DELETE_MAX_AGE_SECS + 60;
    let messages = ctx
        .channel_id()
        .messages(ctx, serenity::GetMessages::new().limit(amount))
        .await?;
    let total = messages.len();
    let ids: Vec<serenity::MessageId> = messages
        .into_iter()
        .filter(|m| m.timestamp.unix_timestamp() > cutoff)
        .map(|m| m.id)
        .collect();

    match ids.as_slice() {
        [] => {}
        [single] => ctx.channel_id().delete_message(ctx, *single).await?,
        _ => ctx.channel_id().delete_messages(ctx, &ids).await?,
    }

    let skipped = total - ids.len();
    let mut text = format!("ลบไป **{}** ข้อความ", ids.len());
    if skipped > 0 {
        text.push_str(&format!(" (ข้าม {skipped} ข้อความที่เก่ากว่า 14 วัน)"));
    }

    let reply = ctx
        .send(CreateReply::default().content(text).ephemeral(true))
        .await?;
    // คำสั่งแบบ prefix: ลบข้อความแจ้งผลหลัง 5 วินาที
    if let poise::Context::Prefix(_) = ctx {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = reply.delete(ctx).await;
    }
    Ok(())
}

/// เตะสมาชิกออกจากเซิร์ฟเวอร์
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "ดูแลเซิร์ฟเวอร์",
    required_permissions = "KICK_MEMBERS",
    required_bot_permissions = "KICK_MEMBERS"
)]
pub async fn kick(
    ctx: Context<'_>,
    #[description = "สมาชิกที่จะเตะ"] user: serenity::User,
    #[description = "เหตุผล"]
    #[rest]
    reason: Option<String>,
) -> Result<(), Error> {
    check_hierarchy(ctx, &user).await?;
    let reason = clean_reason(reason)?;
    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;

    notify(ctx, &user, "เตะออก", &reason).await;
    guild_id.kick_with_reason(ctx, user.id, &reason).await?;

    ctx.say(format!("เตะ **{}** ออกแล้ว\nเหตุผล: {reason}", user.tag()))
        .await?;
    Ok(())
}

/// แบนผู้ใช้ออกจากเซิร์ฟเวอร์
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "ดูแลเซิร์ฟเวอร์",
    required_permissions = "BAN_MEMBERS",
    required_bot_permissions = "BAN_MEMBERS"
)]
pub async fn ban(
    ctx: Context<'_>,
    #[description = "ผู้ใช้ที่จะแบน"] user: serenity::User,
    #[description = "ลบข้อความย้อนหลังกี่วัน (0-7)"]
    #[max = 7]
    delete_days: Option<u8>,
    #[description = "เหตุผล"]
    #[rest]
    reason: Option<String>,
) -> Result<(), Error> {
    check_hierarchy(ctx, &user).await?;
    let reason = clean_reason(reason)?;
    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;
    let delete_days = delete_days.unwrap_or(0).min(7);

    notify(ctx, &user, "แบน", &reason).await;
    guild_id
        .ban_with_reason(ctx, user.id, delete_days, &reason)
        .await?;

    ctx.say(format!("แบน **{}** แล้ว\nเหตุผล: {reason}", user.tag()))
        .await?;
    Ok(())
}

/// ยกเลิกการแบนผู้ใช้ (ใส่ ID ของผู้ใช้)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "ดูแลเซิร์ฟเวอร์",
    required_permissions = "BAN_MEMBERS",
    required_bot_permissions = "BAN_MEMBERS"
)]
pub async fn unban(
    ctx: Context<'_>,
    #[description = "ID ของผู้ใช้ที่จะปลดแบน"] user: serenity::User,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;
    guild_id.unban(ctx, user.id).await?;
    ctx.say(format!("ปลดแบน **{}** แล้ว", user.tag())).await?;
    Ok(())
}

/// ปิดปากสมาชิกชั่วคราว (timeout)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "ดูแลเซิร์ฟเวอร์",
    aliases("mute"),
    required_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS"
)]
pub async fn timeout(
    ctx: Context<'_>,
    #[description = "สมาชิกที่จะ timeout"] user: serenity::User,
    #[description = "ระยะเวลา (นาที) สูงสุด 40320 = 28 วัน"]
    #[min = 1]
    #[max = 40320]
    minutes: u32,
    #[description = "เหตุผล"]
    #[rest]
    reason: Option<String>,
) -> Result<(), Error> {
    if !(1..=MAX_TIMEOUT_MINUTES).contains(&minutes) {
        return Err(format!("ระยะเวลาต้องอยู่ระหว่าง 1-{MAX_TIMEOUT_MINUTES} นาที").into());
    }
    check_hierarchy(ctx, &user).await?;
    let reason = clean_reason(reason)?;
    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;

    let until_unix = serenity::Timestamp::now().unix_timestamp() + i64::from(minutes) * 60;
    let until = serenity::Timestamp::from_unix_timestamp(until_unix)?;
    guild_id
        .edit_member(
            ctx,
            user.id,
            serenity::EditMember::new()
                .disable_communication_until_datetime(until)
                .audit_log_reason(&reason),
        )
        .await?;

    ctx.say(format!(
        "timeout {} ถึง <t:{until_unix}:F> (<t:{until_unix}:R>)\nเหตุผล: {reason}",
        user.mention()
    ))
    .await?;
    Ok(())
}

/// ยกเลิก timeout ของสมาชิก
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "ดูแลเซิร์ฟเวอร์",
    aliases("unmute"),
    required_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS"
)]
pub async fn untimeout(
    ctx: Context<'_>,
    #[description = "สมาชิกที่จะยกเลิก timeout"] user: serenity::User,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;
    guild_id
        .edit_member(
            ctx,
            user.id,
            serenity::EditMember::new().enable_communication(),
        )
        .await?;
    ctx.say(format!("ยกเลิก timeout {} แล้ว", user.mention()))
        .await?;
    Ok(())
}
