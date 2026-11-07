use std::sync::Arc;

use poise::CreateReply;
use songbird::tracks::Track;

use crate::music::{self, MAX_QUEUE, Source, TrackMeta, format_time};
use crate::{Context, Error};

/// เปิดเพลงจาก YouTube (ใส่ลิงก์หรือชื่อเพลง) บอทจะเข้าห้องเสียงให้เอง
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "เพลง",
    aliases("p"),
    user_cooldown = 3
)]
pub async fn play(
    ctx: Context<'_>,
    #[description = "ลิงก์ YouTube หรือชื่อเพลงที่ต้องการค้นหา"]
    #[rest]
    query: String,
) -> Result<(), Error> {
    // ตรวจข้อมูลก่อน จะได้ตอบ error ได้ทันทีโดยไม่ต้องเข้าห้อง
    let source = music::parse_source(&query)?;
    let state = &ctx.data().music;
    state.ensure_ytdlp().await?;

    // yt-dlp ใช้เวลาหลายวินาที ต้อง defer ไม่งั้น slash command จะหมดเวลา
    ctx.defer().await?;
    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;
    let call = music::join_author(ctx).await?;
    let queue_full = || format!("คิวเต็มแล้ว (สูงสุด {MAX_QUEUE} เพลง)");
    if call.lock().await.queue().len() >= MAX_QUEUE {
        return Err(queue_full().into());
    }

    let mut lookup = match source {
        Source::Url(url) => state.ytdl_url(url),
        Source::Search(q) => state.ytdl_search(q),
    };
    let meta = Arc::new(TrackMeta::from_aux(
        state.fetch_metadata(&mut lookup).await?,
        ctx.author().id,
    )?);

    // ดึงเสียงจากลิงก์จริงของวิดีโอ (กันผลค้นหาเปลี่ยนระหว่างรอคิว)
    let input = state.stream(meta.url.clone());
    let track = Track::new_with_data(input, meta.clone()).volume(state.volume(guild_id));

    let position = {
        let mut handler = call.lock().await;
        if handler.queue().len() >= MAX_QUEUE {
            return Err(queue_full().into());
        }
        handler.enqueue(track).await;
        handler.queue().len()
    };

    let reply = if position <= 1 {
        CreateReply::default()
            .embed(music::now_playing_embed(&meta, "กำลังเล่น"))
            .components(music::controls())
    } else {
        let title = format!("เพิ่มเข้าคิวลำดับที่ {}", position - 1);
        CreateReply::default().embed(music::now_playing_embed(&meta, &title))
    };
    ctx.send(reply).await?;
    Ok(())
}

/// ข้ามเพลงที่กำลังเล่น
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "เพลง",
    aliases("s", "next")
)]
pub async fn skip(ctx: Context<'_>) -> Result<(), Error> {
    let queue = music::require_same_channel(ctx).await?;
    ctx.say(music::skip(&queue)?).await?;
    Ok(())
}

/// หยุดชั่วคราว / เล่นต่อ
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "เพลง",
    aliases("resume", "unpause")
)]
pub async fn pause(ctx: Context<'_>) -> Result<(), Error> {
    let queue = music::require_same_channel(ctx).await?;
    ctx.say(music::toggle_pause(&queue).await?).await?;
    Ok(())
}

/// หยุดเล่นและล้างคิวทั้งหมด
#[poise::command(slash_command, prefix_command, guild_only, category = "เพลง")]
pub async fn stop(ctx: Context<'_>) -> Result<(), Error> {
    let queue = music::require_same_channel(ctx).await?;
    ctx.say(music::stop(&queue)?).await?;
    Ok(())
}

/// ดูคิวเพลง
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "เพลง",
    aliases("q")
)]
pub async fn queue(ctx: Context<'_>) -> Result<(), Error> {
    let queue = music::require_same_channel(ctx).await?;
    ctx.send(CreateReply::default().embed(music::queue_embed(&queue)?))
        .await?;
    Ok(())
}

/// ดูเพลงที่กำลังเล่น พร้อมปุ่มควบคุม
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "เพลง",
    aliases("np", "now")
)]
pub async fn nowplaying(ctx: Context<'_>) -> Result<(), Error> {
    let queue = music::require_same_channel(ctx).await?;
    let current = queue.current().ok_or("ตอนนี้ไม่มีเพลงที่กำลังเล่น")?;
    let info = current.get_info().await?;
    let meta = music::meta(&current);

    let progress = match meta.duration {
        Some(total) => {
            let ratio = (info.position.as_secs_f64() / total.as_secs_f64()).clamp(0.0, 1.0);
            let filled = (ratio * 20.0).round() as usize;
            format!(
                "`[{}{}]` {} / {}",
                "=".repeat(filled),
                "-".repeat(20 - filled),
                format_time(info.position),
                format_time(total)
            )
        }
        None => format!("ไลฟ์สด • เล่นมาแล้ว {}", format_time(info.position)),
    };
    let status = match info.playing {
        songbird::tracks::PlayMode::Pause => "หยุดชั่วคราว",
        _ => "กำลังเล่น",
    };
    let looping = if music::is_looping(&info.loops) {
        "เปิด"
    } else {
        "ปิด"
    };

    let embed = music::now_playing_embed(&meta, status)
        .description(format!("{}\n\n{progress}", meta.link()))
        .field(
            "ระดับเสียง",
            format!("{}%", (info.volume * 100.0).round()),
            true,
        )
        .field("วนซ้ำ", looping, true);
    ctx.send(
        CreateReply::default()
            .embed(embed)
            .components(music::controls()),
    )
    .await?;
    Ok(())
}

/// ปรับระดับเสียง 0-200% (ไม่ใส่ค่า = ดูระดับปัจจุบัน)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "เพลง",
    aliases("vol")
)]
pub async fn volume(
    ctx: Context<'_>,
    #[description = "ระดับเสียง 0-200"]
    #[min = 0]
    #[max = 200]
    percent: Option<u16>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;
    let state = &ctx.data().music;

    let Some(percent) = percent else {
        let current = (state.volume(guild_id) * 100.0).round();
        ctx.say(format!("ระดับเสียงตอนนี้: **{current}%**")).await?;
        return Ok(());
    };
    if percent > 200 {
        return Err("ระดับเสียงต้องอยู่ระหว่าง 0-200".into());
    }

    let queue = music::require_same_channel(ctx).await?;
    let volume = f32::from(percent) / 100.0;
    state.set_volume(guild_id, volume);
    // ปรับทุกเพลงในคิว (เพลงที่จบไปแล้วจะข้ามไปเฉยๆ)
    for handle in queue.current_queue() {
        let _ = handle.set_volume(volume);
    }
    ctx.say(format!("ตั้งระดับเสียงเป็น **{percent}%** แล้ว")).await?;
    Ok(())
}

/// เปิด/ปิดการวนเล่นเพลงปัจจุบัน
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "เพลง",
    rename = "loop",
    aliases("repeat")
)]
pub async fn loop_track(ctx: Context<'_>) -> Result<(), Error> {
    let queue = music::require_same_channel(ctx).await?;
    ctx.say(music::toggle_loop(&queue).await?).await?;
    Ok(())
}

/// สลับลำดับเพลงในคิวแบบสุ่ม
#[poise::command(slash_command, prefix_command, guild_only, category = "เพลง")]
pub async fn shuffle(ctx: Context<'_>) -> Result<(), Error> {
    use rand::seq::SliceRandom;

    let queue = music::require_same_channel(ctx).await?;
    if queue.len() < 3 {
        return Err("ต้องมีเพลงรอในคิวอย่างน้อย 2 เพลงถึงจะสลับได้".into());
    }
    queue.modify_queue(|q| {
        // เพลงแรกคือเพลงที่กำลังเล่น ไม่สลับ
        let mut upcoming: Vec<_> = q.drain(1..).collect();
        upcoming.shuffle(&mut rand::rng());
        q.extend(upcoming);
    });
    ctx.say(format!("สลับลำดับ {} เพลงในคิวแล้ว", queue.len() - 1))
        .await?;
    Ok(())
}

/// ลบเพลงออกจากคิว (ดูลำดับจาก /queue)
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "เพลง",
    aliases("rm")
)]
pub async fn remove(
    ctx: Context<'_>,
    #[description = "ลำดับเพลงในคิว (1 = เพลงถัดไป)"]
    #[min = 1]
    position: usize,
) -> Result<(), Error> {
    let queue = music::require_same_channel(ctx).await?;
    let upcoming = queue.len().saturating_sub(1);
    if upcoming == 0 {
        return Err("ไม่มีเพลงรออยู่ในคิว".into());
    }
    if position == 0 || position > upcoming {
        return Err(format!("ลำดับต้องอยู่ระหว่าง 1-{upcoming}").into());
    }
    let handle = queue
        .dequeue(position)
        .ok_or("ไม่พบเพลงลำดับนี้ (คิวอาจเปลี่ยนไปแล้ว)")?
        .handle();
    // dequeue แค่เอาออกจากคิว ต้องสั่งหยุดเพื่อคืนทรัพยากรด้วย
    let _ = handle.stop();
    let title = crate::commands::escape(&music::meta(&handle).title);
    ctx.say(format!("ลบ **{title}** ออกจากคิวแล้ว")).await?;
    Ok(())
}

/// ให้บอทออกจากห้องเสียงและล้างคิว
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "เพลง",
    aliases("dc", "disconnect")
)]
pub async fn leave(ctx: Context<'_>) -> Result<(), Error> {
    let queue = music::require_same_channel(ctx).await?;
    queue.stop();
    let guild_id = ctx.guild_id().ok_or("ใช้ได้ในเซิร์ฟเวอร์เท่านั้น")?;
    music::voice_manager(ctx.serenity_context())
        .await?
        .remove(guild_id)
        .await?;
    ctx.say("ออกจากห้องเสียงแล้ว").await?;
    Ok(())
}
