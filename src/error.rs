use crate::commands::truncate;
use crate::{Data, Error};

/// จัดการ error จากทุกคำสั่ง แล้วตอบกลับผู้ใช้เป็นภาษาไทย
pub async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
    use poise::FrameworkError as E;

    let (ctx, message) = match error {
        E::Setup { error, .. } => {
            eprintln!("[ผิดพลาด] ตั้งค่าบอทไม่สำเร็จ: {error}");
            return;
        }
        E::EventHandler { error, event, .. } => {
            eprintln!(
                "[ผิดพลาด] จัดการ event {} ไม่สำเร็จ: {error}",
                event.snake_case_name()
            );
            return;
        }
        E::Command { error, ctx, .. } => {
            eprintln!(
                "[ผิดพลาด] คำสั่ง {} ผิดพลาด: {error}",
                ctx.command().qualified_name
            );
            (ctx, error.to_string())
        }
        E::CommandPanic { payload, ctx, .. } => {
            eprintln!(
                "[ร้ายแรง] คำสั่ง {} panic: {}",
                ctx.command().qualified_name,
                payload.as_deref().unwrap_or("ไม่ทราบสาเหตุ")
            );
            (
                ctx,
                "เกิดข้อผิดพลาดภายในบอท ลองใหม่อีกครั้ง ถ้ายังไม่ได้ให้แจ้งผู้ดูแล".to_string(),
            )
        }
        E::ArgumentParse { ctx, input, .. } => {
            let usage = format!("{}help {}", ctx.data().prefix, ctx.command().name);
            let detail = input
                .map(|i| format!(" (`{}`)", truncate(&i, 100)))
                .unwrap_or_default();
            (ctx, format!("รูปแบบคำสั่งไม่ถูกต้อง{detail} ดูวิธีใช้ด้วย `{usage}`"))
        }
        E::MissingUserPermissions {
            ctx,
            missing_permissions,
            ..
        } => {
            let perms = missing_permissions
                .map(|p| format!(": {p}"))
                .unwrap_or_default();
            (ctx, format!("คุณไม่มีสิทธิ์ใช้คำสั่งนี้{perms}"))
        }
        E::MissingBotPermissions {
            ctx,
            missing_permissions,
            ..
        } => (ctx, format!("บอทขาดสิทธิ์ที่จำเป็น: {missing_permissions}")),
        E::GuildOnly { ctx, .. } => (ctx, "คำสั่งนี้ใช้ได้ในเซิร์ฟเวอร์เท่านั้น".to_string()),
        E::CooldownHit {
            ctx,
            remaining_cooldown,
            ..
        } => (
            ctx,
            format!(
                "ใจเย็นๆ รออีก {:.1} วินาทีค่อยใช้ใหม่",
                remaining_cooldown.as_secs_f32()
            ),
        ),
        other => {
            // error ประเภทอื่นให้ poise จัดการตามค่าเริ่มต้น
            if let Err(e) = poise::builtins::on_error(other).await {
                eprintln!("[ผิดพลาด] จัดการ error ไม่สำเร็จ: {e}");
            }
            return;
        }
    };

    // Discord จำกัดข้อความไว้ 2000 ตัวอักษร
    let reply = poise::CreateReply::default()
        .content(truncate(&message, 1900))
        .ephemeral(true);
    if let Err(e) = ctx.send(reply).await {
        eprintln!("[ผิดพลาด] ส่งข้อความแจ้ง error ไม่สำเร็จ: {e}");
    }
}
