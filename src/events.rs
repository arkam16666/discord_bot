use poise::serenity_prelude as serenity;

use crate::{Data, Error, members, music};

/// รับ event จาก Discord ที่ไม่ใช่คำสั่ง
pub async fn handle(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    _framework: poise::FrameworkContext<'_, Data, Error>,
    data: &Data,
) -> Result<(), Error> {
    match event {
        // กดปุ่มควบคุมเพลง
        serenity::FullEvent::InteractionCreate {
            interaction: serenity::Interaction::Component(press),
        } => music::handle_button(ctx, press).await?,
        serenity::FullEvent::GuildMemberAddition { new_member } => {
            members::on_join(ctx, data, new_member).await?;
        }
        serenity::FullEvent::GuildMemberRemoval {
            guild_id,
            user,
            member_data_if_available,
        } => {
            let member = member_data_if_available.as_ref();
            members::on_leave(ctx, data, *guild_id, user, member).await?;
        }
        serenity::FullEvent::GuildCreate { guild, is_new } => {
            members::request_members(ctx, guild, data.features);
            if *is_new == Some(true) {
                println!("[เซิร์ฟเวอร์] ถูกเชิญเข้าเซิร์ฟเวอร์ใหม่: {}", guild.name);
            }
        }
        _ => {}
    }
    Ok(())
}
