use std::time::Duration;

use poise::CreateReply;
use poise::serenity_prelude as serenity;
use rand::Rng;
use rand::seq::IndexedRandom;

use super::{COLOR, truncate};
use crate::{Context, Error};

/// ทอยลูกเต๋า เช่น 1d6, 2d20 (จำนวนลูก d จำนวนหน้า)
#[poise::command(slash_command, prefix_command, category = "สนุกๆ", aliases("dice"))]
pub async fn roll(
    ctx: Context<'_>,
    #[description = "รูปแบบ XdY เช่น 2d6 (ค่าเริ่มต้น 1d6)"] dice: Option<String>,
) -> Result<(), Error> {
    let dice = dice.unwrap_or_else(|| "1d6".to_string());
    let (count, sides) =
        parse_dice(&dice).ok_or("รูปแบบไม่ถูกต้อง ใช้แบบ `2d6` (ทอยได้ 1-100 ลูก, 2-1000 หน้า)")?;

    // สร้างตัวเลขสุ่มให้เสร็จก่อน .await (ThreadRng ส่งข้าม thread ไม่ได้)
    let rolls: Vec<u32> = {
        let mut rng = rand::rng();
        (0..count).map(|_| rng.random_range(1..=sides)).collect()
    };
    let total: u32 = rolls.iter().sum();

    let detail = if count == 1 {
        String::new()
    } else {
        let shown: Vec<String> = rolls.iter().take(30).map(u32::to_string).collect();
        let more = if count > 30 { " ..." } else { "" };
        format!("\n[{}{}]", shown.join(", "), more)
    };
    ctx.say(format!("ทอย **{count}d{sides}** ได้ **{total}**{detail}"))
        .await?;
    Ok(())
}

/// แยก "2d6" เป็น (2, 6) พร้อมตรวจขอบเขต
fn parse_dice(input: &str) -> Option<(u32, u32)> {
    let input = input.trim().to_lowercase();
    let (count, sides) = input.split_once('d')?;
    let count = if count.is_empty() {
        1
    } else {
        count.parse().ok()?
    };
    let sides = sides.parse().ok()?;
    ((1..=100).contains(&count) && (2..=1000).contains(&sides)).then_some((count, sides))
}

/// โยนเหรียญ หัวหรือก้อย
#[poise::command(slash_command, prefix_command, category = "สนุกๆ", aliases("flip"))]
pub async fn coinflip(ctx: Context<'_>) -> Result<(), Error> {
    let result = if rand::random::<bool>() {
        "ออก **หัว**!"
    } else {
        "ออก **ก้อย**!"
    };
    ctx.say(result).await?;
    Ok(())
}

const EIGHTBALL_ANSWERS: &[&str] = &[
    "แน่นอน 100%",
    "ใช่เลย",
    "ไม่ต้องสงสัย",
    "น่าจะใช่นะ",
    "ดูท่าจะดี",
    "สัญญาณบอกว่าใช่",
    "ถามใหม่อีกทีนะ",
    "ตอนนี้ยังบอกไม่ได้",
    "ตั้งสมาธิแล้วถามใหม่",
    "อย่าหวังเลย",
    "คำตอบคือไม่",
    "แหล่งข่าวบอกว่าไม่",
    "ไม่น่าจะใช่",
    "สงสัยจะไม่รอด",
];

/// ถามลูกแก้ววิเศษ 8-ball
#[poise::command(
    slash_command,
    prefix_command,
    category = "สนุกๆ",
    rename = "8ball",
    aliases("eightball")
)]
pub async fn eightball(
    ctx: Context<'_>,
    #[description = "คำถามของคุณ"]
    #[rest]
    question: String,
) -> Result<(), Error> {
    let answer = EIGHTBALL_ANSWERS
        .choose(&mut rand::rng())
        .copied()
        .unwrap_or("...");
    let embed = serenity::CreateEmbed::new()
        .color(COLOR)
        .field("คำถาม", truncate(&question, 1000), false)
        .field("คำตอบ", answer, false);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// ให้บอทช่วยเลือก (คั่นตัวเลือกด้วย , หรือ |)
#[poise::command(slash_command, prefix_command, category = "สนุกๆ", aliases("pick"))]
pub async fn choose(
    ctx: Context<'_>,
    #[description = "ตัวเลือก คั่นด้วย , หรือ | เช่น ข้าวมันไก่, กะเพรา, ส้มตำ"]
    #[rest]
    choices: String,
) -> Result<(), Error> {
    let options: Vec<&str> = choices
        .split([',', '|'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if options.len() < 2 {
        return Err("ต้องมีอย่างน้อย 2 ตัวเลือก คั่นด้วย , หรือ |".into());
    }
    let picked = options
        .choose(&mut rand::rng())
        .copied()
        .unwrap_or_default();
    ctx.say(format!("ขอเลือก... **{}**!", truncate(picked, 1500)))
        .await?;
    Ok(())
}

/// ข้อจำกัดของโพลใน Discord
const POLL_MAX_ANSWERS: usize = 10;
const POLL_QUESTION_MAX: usize = 300;
const POLL_ANSWER_MAX: usize = 55;
const POLL_MAX_HOURS: u32 = 768; // 32 วัน

/// สร้างโพลโหวตของ Discord (ไม่ใส่ตัวเลือก = โพล ใช่/ไม่ใช่)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "สนุกๆ",
    user_cooldown = 10,
    required_bot_permissions = "SEND_MESSAGES | SEND_POLLS"
)]
pub async fn poll(
    ctx: Context<'_>,
    #[description = "เปิดโหวตกี่ชั่วโมง (1-768, ค่าเริ่มต้น 24)"]
    #[min = 1]
    #[max = 768]
    hours: Option<u32>,
    #[description = "คำถาม | ตัวเลือก1 | ตัวเลือก2 ... (สูงสุด 10 ตัวเลือก)"]
    #[rest]
    input: String,
) -> Result<(), Error> {
    let hours = hours.unwrap_or(24);
    if !(1..=POLL_MAX_HOURS).contains(&hours) {
        return Err(format!("ระยะเวลาต้องอยู่ระหว่าง 1-{POLL_MAX_HOURS} ชั่วโมง").into());
    }

    let mut parts = input.split('|').map(str::trim).filter(|s| !s.is_empty());
    let question = parts.next().ok_or("ต้องใส่คำถาม")?;
    let mut answers: Vec<&str> = parts.collect();

    if question.chars().count() > POLL_QUESTION_MAX {
        return Err(format!("คำถามยาวได้ไม่เกิน {POLL_QUESTION_MAX} ตัวอักษร").into());
    }
    if answers.len() == 1 {
        return Err("ต้องมีอย่างน้อย 2 ตัวเลือก (หรือไม่ใส่เลยเพื่อทำโพล ใช่/ไม่ใช่)".into());
    }
    if answers.len() > POLL_MAX_ANSWERS {
        return Err(format!("ใส่ตัวเลือกได้สูงสุด {POLL_MAX_ANSWERS} ตัว").into());
    }
    if let Some(long) = answers.iter().find(|a| a.chars().count() > POLL_ANSWER_MAX) {
        return Err(format!(
            "ตัวเลือก \"{}\" ยาวเกิน {POLL_ANSWER_MAX} ตัวอักษร",
            truncate(long, 60)
        )
        .into());
    }
    if answers.is_empty() {
        answers = vec!["ใช่", "ไม่ใช่"];
    }

    let poll = serenity::CreatePoll::new()
        .question(question)
        .answers(
            answers
                .iter()
                .map(|a| serenity::CreatePollAnswer::new().text(*a))
                .collect(),
        )
        .duration(Duration::from_secs(u64::from(hours) * 3600));

    // แบบ prefix: ลบข้อความคำสั่งทิ้ง เหลือแต่โพล
    if let poise::Context::Prefix(prefix_ctx) = ctx {
        let _ = prefix_ctx.msg.delete(ctx).await;
    }

    ctx.channel_id()
        .send_message(ctx, serenity::CreateMessage::new().poll(poll))
        .await?;

    if let poise::Context::Application(_) = ctx {
        ctx.send(CreateReply::default().content("สร้างโพลแล้ว").ephemeral(true))
            .await?;
    }
    Ok(())
}

/// ให้บอทพูดตามที่พิมพ์ (ต้องมีสิทธิ์จัดการข้อความ)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "สนุกๆ",
    required_permissions = "MANAGE_MESSAGES"
)]
pub async fn say(
    ctx: Context<'_>,
    #[description = "ข้อความที่ให้บอทพูด"]
    #[rest]
    text: String,
) -> Result<(), Error> {
    if text.chars().count() > 2000 {
        return Err("ข้อความยาวได้ไม่เกิน 2000 ตัวอักษร".into());
    }

    // ปิดการ mention ทั้งหมด กันคนใช้บอทแท็ก @everyone
    let no_mentions = serenity::CreateAllowedMentions::new();

    // แบบ prefix: ลบข้อความคำสั่งทิ้ง ให้ดูเหมือนบอทพูดเอง
    if let poise::Context::Prefix(prefix_ctx) = ctx {
        let _ = prefix_ctx.msg.delete(ctx).await;
    }

    ctx.channel_id()
        .send_message(
            ctx,
            serenity::CreateMessage::new()
                .content(text)
                .allowed_mentions(no_mentions),
        )
        .await?;

    // แบบ slash: ต้องตอบ interaction ไม่งั้น Discord จะขึ้นว่าล้มเหลว
    if let poise::Context::Application(_) = ctx {
        ctx.send(CreateReply::default().content("ส่งแล้ว").ephemeral(true))
            .await?;
    }
    Ok(())
}
